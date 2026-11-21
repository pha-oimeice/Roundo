//! GPU-driven voxel-chunk extraction, residency, LOD, geometry generation, and drawing.
//!
//! Main-world chunks are extracted as immutable inputs to an event-driven SVO
//! resource worker. Packing, upload completion and SVO retirement do not depend on
//! frames. GPU measurement, view-dependent LOD selection, paged allocation and
//! generation precede drawing without a model-count readback. Only completed
//! geometry is published; retired pages are reused in queue-ordered GPU passes.

mod compute;
mod draw;
mod model;
mod residency;
mod shaders;
use compute::*;
use draw::*;
use model::*;
use residency::*;
use shaders::*;

mod arena;
mod ranges;
mod surface_proxy;
mod svo_arena;
mod svo_upload;

use arena::{GeometryArena, GpuPagePoolBinding};
use surface_proxy::{GpuQefReducePass, GpuQefSummary};
use svo_arena::SvoAllocation;
use svo_upload::SvoUploads;

use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        core_3d::main_opaque_pass_3d,
        schedule::{Core3d, Core3dSystems},
    },
    prelude::{
        App, Commands, Component, DetectChanges, Entity, GlobalTransform, IVec4,
        IntoScheduleConfigs, Mat4, Query, Ref, Res, ResMut, Resource, UVec4,
    },
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BlendState, Buffer, BufferInitDescriptor, BufferUsages, CachedComputePipelineId,
            CachedRenderPipelineId, ColorTargetState, ColorWrites, CompareFunction,
            ComputePassDescriptor, ComputePipelineDescriptor, DepthStencilState, Face,
            FragmentState, FrontFace, MultisampleState, PipelineCache, PolygonMode,
            PrimitiveState, PrimitiveTopology, RenderPassDescriptor, RenderPipelineDescriptor,
            ShaderStages, ShaderType, StencilState, StoreOp, TextureFormat, VertexState,
            binding_types::{storage_buffer, storage_buffer_read_only, uniform_buffer},
        },
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        view::{
            ExtractedView, Msaa, ViewDepthStencilTexture, ViewTarget, ViewUniform,
            ViewUniformOffset, ViewUniforms,
        },
    },
};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, ChunkId, LocalCoordinate, LocalCoordinateId, LocalCoordinateIdentity,
    VoxelChunkSvo,
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

/// Default deterministic seed used by the voxel material shader.
pub const DEFAULT_WORLD_MATERIAL_SEED: u32 = 0xDEAD_BEEF;
const DEFAULT_GEOMETRY_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_SVO_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const CHUNK_EDGE: u32 = CHUNK_EDGE_LENGTH as u32;
const FACE_PLANE_COUNT: u32 = 6 * CHUNK_EDGE;
const MAX_CHUNK_LOD: u8 = CHUNK_EDGE.ilog2() as u8;
const GEOMETRY_LOD_COUNT: usize = MAX_CHUNK_LOD as usize + 1;
const MODEL_INFO_BYTES: u64 = GEOMETRY_LOD_COUNT as u64 * 16;
const MESH_WORKGROUPS: u32 = FACE_PLANE_COUNT + 512 + 64 + 8 + 1;
// The GPU ABI and dispatch topology below intentionally model one fixed 16³ cube.
// Fail compilation instead of silently drifting into a 16²×height representation.
const _: () = assert!(CHUNK_EDGE == 16 && MAX_CHUNK_LOD == 4);
const GENERATED_QUAD_SIZE: u64 = 16;
// Fixed descriptor capacity keeps GPU visibility ownership independent from
// resident-set size changes. A paged descriptor arena will replace this limit.
const MAX_GPU_CHUNK_DESCRIPTORS: u32 = 262_144;
const MAX_GPU_GEOMETRY_SEGMENTS: u32 = 16;
/// Render-world budgets and deterministic material input.
///
/// Configure the geometry budget before renderer initialization: existing arena
/// segments retain their initial total-reservation limit. Pixel error is a live
/// selection policy and may change without rebuilding the SVO measurements.
#[derive(Clone, Copy, Debug, ExtractResource, Resource)]
#[extract_app(RenderApp)]
pub struct WorldRenderSettings {
    /// Seed mixed into deterministic material/color selection for newly prepared chunks.
    pub material_seed: u32,
    /// Maximum total bytes reserved by retained geometry-arena segments.
    pub geometry_budget_bytes: u64,
    /// Maximum conservative surface displacement in physical viewport pixels.
    /// Zero permits only exact geometry. Invalid values select exact geometry.
    pub geometry_lod_pixel_error: f32,
}

impl Default for WorldRenderSettings {
    fn default() -> Self {
        Self {
            material_seed: DEFAULT_WORLD_MATERIAL_SEED,
            geometry_budget_bytes: DEFAULT_GEOMETRY_BUDGET_BYTES,
            geometry_lod_pixel_error: 1.0,
        }
    }
}

/// Installs extraction and render-graph systems for local voxel chunks.
///
/// If Bevy has no [`RenderApp`], the plugin still initializes settings and
/// shader assets but installs no GPU-world systems.
pub struct WorldRenderPlugin;

impl bevy::prelude::Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        register_world_shaders(app);

        app.init_resource::<WorldRenderSettings>()
            .add_plugins(ExtractResourcePlugin::<WorldRenderSettings>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<ExtractedChunks>()
            .init_resource::<WorldResidency>()
            .init_resource::<WorldViewPipelines>()
            .init_resource::<WorldShaderInitialization>()
            .add_systems(RenderStartup, init_world_pipelines)
            .add_systems(ExtractSchedule, extract_loaded_chunks)
            .add_systems(
                Render,
                (
                    prepare_residency.run_if(world_shaders_ready),
                    prepare_view_pipelines,
                )
                    .chain()
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Render,
                prepare_view_bind_group.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(
                Render,
                check_world_shader_initialization
                    .after(RenderSystems::Prepare)
                    .before(RenderSystems::Render),
            )
            .add_systems(
                Core3d,
                world_compute
                    .run_if(world_shaders_ready)
                    .in_set(Core3dSystems::Prepass),
            )
            .add_systems(
                Core3d,
                world_draw
                    .run_if(world_shaders_ready)
                    .in_set(Core3dSystems::MainPass)
                    .before(main_opaque_pass_3d),
            );
    }
}

#[derive(Resource, Default)]
struct ExtractedChunks {
    epoch: u64,
    changes: Vec<ExtractedChunkChange>,
    tracked: HashMap<LocalCoordinateId, HashMap<[i64; 3], u64>>,
}

enum ExtractedChunkChange {
    Upsert(ExtractedChunk),
    Transform {
        local_coordinate_id: LocalCoordinateId,
        world_from_local: [f32; 16],
    },
    Remove(ChunkId),
}

#[derive(Clone)]
struct ExtractedChunk {
    id: ChunkId,
    revision: u64,
    svo: Arc<VoxelChunkSvo>,
    world_from_chunk: [f32; 16],
}

#[derive(Resource, Default)]
struct WorldResidency {
    chunks: HashMap<ChunkId, ResidentChunk>,
    geometry_dirty: HashSet<ChunkId>,
    geometry_arena: Option<GeometryArena>,
    svo_uploads: Option<SvoUploads>,
    svo_buffers: Vec<Buffer>,
    pending_svos: HashMap<ChunkId, PendingSvo>,
    stats: WorldRenderStats,
    gpu_culling: Option<GpuCulling>,
    next_gpu_slot: u32,
    free_gpu_slots: Vec<u32>,
    next_generation: u32,
    next_incarnation: u32,
    pending_status: Option<PendingGpuStatus>,
    gpu_activity: [u32; 4],
    last_compute_epoch: Option<u64>,
}

#[derive(Default)]
struct WorldRenderStats {
    last_report_epoch: u64,
    compute_cpu_micros: u64,
}

struct ResidentChunk {
    revision: u64,
    gpu_slot: u32,
    material_seed: u32,
    coordinate: [i64; 3],
    svo: SvoAllocation,
    generation: u32,
    incarnation: u32,
    world_from_chunk: [f32; 16],
}

struct PendingSvo {
    request: u64,
    observed: ExtractedChunk,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuPackedSvoNode {
    words: UVec4,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuCullDescriptor {
    world_from_chunk: Mat4,
    chunk_coordinate: IVec4,
    draws: [UVec4; MAX_CHUNK_LOD as usize + 1],
    metadata: UVec4,
    svo: UVec4,
    neighbor_slots: [UVec4; 2],
}

#[derive(Clone, Copy, ShaderType)]
struct GpuBuildPass {
    svo_segment: u32,
    geometry_segment: u32,
    descriptor_count: u32,
    reserved: u32,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuBuildRequest {
    target: UVec4,
    bounds: UVec4,
}

/// GPU-owned count generation, committed generation/LOD, and page ownership.
#[derive(Clone, Copy, ShaderType)]
struct GpuModelState {
    generations: UVec4,
    allocation: UVec4,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuModelView {
    clip_from_world: Mat4,
    viewport: UVec4,
}

struct GpuDrawLayer {
    visible_draws: Buffer,
    segment_counts: Buffer,
    bind_group: BindGroup,
}

struct GpuCulling {
    descriptors: Buffer,
    quad_counts: Buffer,
    boundary_rows: Buffer,
    models: Buffer,
    count_requests: Buffer,
    emit_requests: Buffer,
    emit_dispatch: Buffer,
    dummy_geometry: Buffer,
    count_bind_groups: Vec<BindGroup>,
    build_bind_groups: Vec<Vec<BindGroup>>,
    draw_layers: Vec<GpuDrawLayer>,
    draw_capacity: u32,
    render_segment_bind_groups: Vec<BindGroup>,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuGeneratedQuad {
    data: UVec4,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuDrawIndirect {
    words: UVec4,
}

#[derive(Resource)]
struct WorldPipelines {
    compute_layout: BindGroupLayoutDescriptor,
    view_layout: BindGroupLayoutDescriptor,
    render_layout: BindGroupLayoutDescriptor,
    prepare_boundary: CachedComputePipelineId,
    reset_geometry: CachedComputePipelineId,
    count_models: CachedComputePipelineId,
    gather_counts: CachedComputePipelineId,
    reclaim_models: CachedComputePipelineId,
    reclassify_slabs: CachedComputePipelineId,
    select_models: CachedComputePipelineId,
    finalize_dispatch: CachedComputePipelineId,
    select_layout: BindGroupLayoutDescriptor,
    compute: CachedComputePipelineId,
    finalize_builds: CachedComputePipelineId,
    cull: CachedComputePipelineId,
    cull_layout: BindGroupLayoutDescriptor,
    _surface_proxy_layout: BindGroupLayoutDescriptor,
    _surface_proxy_reduce: CachedComputePipelineId,
    vertex_shader: bevy::prelude::Handle<bevy::shader::Shader>,
    fragment_shader: bevy::prelude::Handle<bevy::shader::Shader>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResidentUpdate {
    Unchanged,
    Transform,
    Replace,
}

fn resident_update(
    resident: Option<(u64, [f32; 16])>,
    observed_revision: u64,
    observed_transform: [f32; 16],
) -> ResidentUpdate {
    match resident {
        Some((revision, transform)) if revision == observed_revision => {
            if transform == observed_transform {
                ResidentUpdate::Unchanged
            } else {
                ResidentUpdate::Transform
            }
        }
        _ => ResidentUpdate::Replace,
    }
}

#[derive(Resource, Default)]
struct WorldViewPipelines(HashMap<(TextureFormat, u32), CachedRenderPipelineId>);

#[derive(Component)]
struct WorldViewPipeline(CachedRenderPipelineId);

#[derive(Resource)]
struct WorldViewBindGroup(BindGroup);
