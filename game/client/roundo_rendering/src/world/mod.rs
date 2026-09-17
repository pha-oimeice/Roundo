//! GPU-driven voxel-chunk extraction, residency, LOD, geometry generation, and drawing.
//!
//! Main-world chunks are copied into render-world snapshots. Geometry replacement
//! is incremental and budgeted: an older complete mesh remains drawable until a
//! replacement has been generated, read back, allocated, and made resident.

mod arena;
#[cfg(test)]
mod lod;
mod svo_arena;

use arena::{GeometryAllocation, GeometryArena};
use svo_arena::{SvoAllocation, SvoArena};

#[cfg(test)]
use bevy::prelude::Vec4;

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
            FragmentState, FrontFace, MultisampleState, PipelineCache, PolygonMode, PrimitiveState,
            PrimitiveTopology, RenderPassDescriptor, RenderPipelineDescriptor, ShaderStages,
            ShaderType, StencilState, StoreOp, TextureFormat, VertexState,
            binding_types::{storage_buffer, storage_buffer_read_only, uniform_buffer},
        },
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        view::{
            ExtractedView, Msaa, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset,
            ViewUniforms,
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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

/// Default deterministic seed used by the voxel material shader.
pub const DEFAULT_WORLD_MATERIAL_SEED: u32 = 0xDEAD_BEEF;
const DEFAULT_GEOMETRY_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_SVO_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const CHUNK_EDGE: u32 = CHUNK_EDGE_LENGTH as u32;
const FACE_PLANE_COUNT: u32 = 6 * CHUNK_EDGE;
const MAX_CHUNK_LOD: u8 = CHUNK_EDGE.ilog2() as u8;
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
/// Callers should configure this resource before the renderer initializes.
/// Runtime changes do not resize existing arena segments; the geometry arena
/// retains the total-reservation limit used when it is first created.
#[derive(Clone, Copy, Debug, ExtractResource, Resource)]
pub struct WorldRenderSettings {
    /// Seed mixed into deterministic material/color selection for newly prepared chunks.
    pub material_seed: u32,
    /// Maximum total bytes reserved by retained geometry-arena segments.
    pub geometry_budget_bytes: u64,
}

impl Default for WorldRenderSettings {
    fn default() -> Self {
        Self {
            material_seed: DEFAULT_WORLD_MATERIAL_SEED,
            geometry_budget_bytes: DEFAULT_GEOMETRY_BUDGET_BYTES,
        }
    }
}

/// Installs extraction and render-graph systems for streamed voxel chunks.
///
/// If Bevy has no [`RenderApp`], the plugin still initializes settings and
/// shader assets but installs no GPU-world systems.
pub struct WorldRenderPlugin;

impl bevy::prelude::Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/world_cull_lod.wgsl");
        embedded_asset!(app, "shaders/world_greedy_emit.wgsl");
        embedded_asset!(app, "shaders/world_vertex.wgsl");
        embedded_asset!(app, "shaders/world_fragment.wgsl");

        app.init_resource::<WorldRenderSettings>()
            .add_plugins(ExtractResourcePlugin::<WorldRenderSettings>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<ExtractedChunks>()
            .init_resource::<WorldResidency>()
            .init_resource::<WorldViewPipelines>()
            .add_systems(RenderStartup, init_world_pipelines)
            .add_systems(ExtractSchedule, extract_loaded_chunks)
            .add_systems(
                Render,
                (prepare_residency, prepare_view_pipelines)
                    .chain()
                    .in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Render,
                prepare_view_bind_group.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(Core3d, world_compute.in_set(Core3dSystems::Prepass))
            .add_systems(
                Core3d,
                world_draw
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

struct ExtractedChunk {
    id: ChunkId,
    revision: u64,
    svo: Arc<VoxelChunkSvo>,
    world_from_chunk: [f32; 16],
}

fn extract_loaded_chunks(
    mut extracted: ResMut<ExtractedChunks>,
    chunks: Extract<
        Query<(
            &LocalCoordinateIdentity,
            Ref<LocalCoordinate>,
            Ref<GlobalTransform>,
        )>,
    >,
) {
    extracted.epoch = extracted.epoch.wrapping_add(1);
    extracted.changes.clear();
    let mut observed_local_coordinates = std::collections::HashSet::new();

    for (identity, local_coordinate, global_transform) in &chunks {
        let local_coordinate_id = identity.0;
        observed_local_coordinates.insert(local_coordinate_id);
        let transform_changed = global_transform.is_changed();
        if transform_changed {
            extracted.changes.push(ExtractedChunkChange::Transform {
                local_coordinate_id,
                world_from_local: global_transform.to_matrix().to_cols_array(),
            });
        }
        if !local_coordinate.is_changed() && extracted.tracked.contains_key(&local_coordinate_id) {
            continue;
        }

        let tracked_chunks = extracted.tracked.remove(&local_coordinate_id);
        let previous = tracked_chunks.unwrap_or_default();
        let mut current = HashMap::new();
        for chunk in local_coordinate.loaded_chunk_views() {
            let coordinate = [
                i64::from(chunk.position.x),
                i64::from(chunk.position.y),
                i64::from(chunk.position.z),
            ];
            current.insert(coordinate, chunk.revision);
            if previous.get(&coordinate) == Some(&chunk.revision) && !transform_changed {
                continue;
            }
            let chunk_translation = bevy::math::Affine3A::from_translation(
                (chunk.position * CHUNK_EDGE_LENGTH as i32).as_vec3(),
            );
            let world_from_chunk = global_transform.affine() * chunk_translation;
            extracted
                .changes
                .push(ExtractedChunkChange::Upsert(ExtractedChunk {
                    id: ChunkId {
                        local_coordinate_id,
                        coordinate,
                    },
                    revision: chunk.revision,
                    svo: Arc::clone(chunk.svo),
                    world_from_chunk: Mat4::from(world_from_chunk).to_cols_array(),
                }));
        }
        for coordinate in previous.keys() {
            if !current.contains_key(coordinate) {
                extracted
                    .changes
                    .push(ExtractedChunkChange::Remove(ChunkId {
                        local_coordinate_id,
                        coordinate: *coordinate,
                    }));
            }
        }
        extracted.tracked.insert(local_coordinate_id, current);
    }

    let removed_local_coordinates = extracted
        .tracked
        .keys()
        .filter(|id| !observed_local_coordinates.contains(id))
        .copied()
        .collect::<Vec<_>>();
    for local_coordinate_id in removed_local_coordinates {
        if let Some(chunks) = extracted.tracked.remove(&local_coordinate_id) {
            extracted
                .changes
                .extend(chunks.into_keys().map(|coordinate| {
                    ExtractedChunkChange::Remove(ChunkId {
                        local_coordinate_id,
                        coordinate,
                    })
                }));
        }
    }
}

#[derive(Resource, Default)]
struct WorldResidency {
    chunks: HashMap<ChunkId, ResidentChunk>,
    local_transforms: HashMap<LocalCoordinateId, [f32; 16]>,
    geometry_dirty: HashSet<ChunkId>,
    geometry_arena: Option<GeometryArena>,
    svo_arena: Option<SvoArena>,
    retired: Vec<RetiredChunk>,
    retired_geometry: Vec<RetiredGeometry>,
    upload_count: u64,
    completed_submission_epoch: Arc<AtomicU64>,
    stats: WorldRenderStats,
    gpu_culling: Option<GpuCulling>,
    next_gpu_slot: u32,
    free_gpu_slots: Vec<u32>,
    slot_chunks: Vec<Option<ChunkId>>,
}

#[derive(Default)]
struct WorldRenderStats {
    last_report_epoch: u64,
    geometry_builds: u64,
    geometry_budget_rejections: u64,
    compute_cpu_micros: u64,
}

struct ResidentChunk {
    revision: u64,
    gpu_slot: u32,
    material_seed: u32,
    coordinate: [i64; 3],
    svo: SvoAllocation,
    geometries: [Option<ResidentGeometry>; MAX_CHUNK_LOD as usize + 1],
    world_from_chunk: [f32; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GeometryKey {
    source_revision: u64,
    lod: u8,
    boundary_signature: u64,
}

struct ResidentGeometry {
    key: GeometryKey,
    allocated_bytes: u64,
    allocation: GeometryAllocation,
}

struct RetiredChunk {
    retired_at_epoch: u64,
    chunk: ResidentChunk,
}

struct RetiredGeometry {
    retired_at_epoch: u64,
    geometry: ResidentGeometry,
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

struct GpuCulling {
    descriptors: Buffer,
    visible_draws: Buffer,
    segment_counts: Buffer,
    quad_counts: Buffer,
    boundary_rows: Buffer,
    bind_group: BindGroup,
    render_segment_bind_groups: Vec<BindGroup>,
    build_bind_groups: Vec<Vec<BindGroup>>,
    build_slots: Buffer,
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
    compute: CachedComputePipelineId,
    finalize_builds: CachedComputePipelineId,
    cull: CachedComputePipelineId,
    cull_layout: BindGroupLayoutDescriptor,
    vertex_shader: bevy::prelude::Handle<bevy::shader::Shader>,
    fragment_shader: bevy::prelude::Handle<bevy::shader::Shader>,
}

fn init_world_pipelines(
    mut commands: Commands,
    asset_server: Res<bevy::prelude::AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let compute_layout = BindGroupLayoutDescriptor::new(
        "world compute",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer::<GpuGeneratedQuad>(false),
                storage_buffer::<GpuCullDescriptor>(false),
                storage_buffer::<u32>(false),
                storage_buffer::<u32>(false),
                uniform_buffer::<GpuBuildPass>(false),
                storage_buffer_read_only::<u32>(false),
            ),
        ),
    );
    let view_layout = BindGroupLayoutDescriptor::new(
        "world view",
        &BindGroupLayoutEntries::single(
            ShaderStages::COMPUTE | ShaderStages::VERTEX,
            uniform_buffer::<ViewUniform>(true),
        ),
    );
    let render_layout = BindGroupLayoutDescriptor::new(
        "world chunk",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            (
                storage_buffer_read_only::<GpuGeneratedQuad>(false),
                storage_buffer_read_only::<GpuCullDescriptor>(false),
            ),
        ),
    );
    let cull_layout = BindGroupLayoutDescriptor::new(
        "world GPU culling",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                storage_buffer::<GpuCullDescriptor>(false),
                storage_buffer::<GpuDrawIndirect>(false),
                storage_buffer::<u32>(false),
            ),
        ),
    );
    let compute_shader =
        load_embedded_asset!(asset_server.as_ref(), "shaders/world_greedy_emit.wgsl");
    let cull_shader = load_embedded_asset!(asset_server.as_ref(), "shaders/world_cull_lod.wgsl");
    let cull = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some(Cow::Borrowed("world parallel Chunk culling")),
        layout: vec![view_layout.clone(), cull_layout.clone()],
        shader: cull_shader,
        entry_point: Some(Cow::Borrowed("main")),
        ..Default::default()
    });
    let vertex_shader = load_embedded_asset!(asset_server.as_ref(), "shaders/world_vertex.wgsl");
    let fragment_shader =
        load_embedded_asset!(asset_server.as_ref(), "shaders/world_fragment.wgsl");
    let build_pipeline = |label: &'static str, entry: &'static str| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(Cow::Borrowed(label)),
            layout: vec![compute_layout.clone()],
            shader: compute_shader.clone(),
            entry_point: Some(Cow::Borrowed(entry)),
            ..Default::default()
        })
    };
    let prepare_boundary = build_pipeline("world SVO boundary summaries", "prepare_boundary");
    let reset_geometry = build_pipeline("world geometry reset", "reset_geometry");
    let compute = build_pipeline("world SVO greedy meshing", "mesh_chunks");
    let finalize_builds = build_pipeline("world geometry commit", "finalize_builds");
    commands.insert_resource(WorldPipelines {
        compute_layout,
        view_layout,
        render_layout,
        prepare_boundary,
        reset_geometry,
        compute,
        finalize_builds,
        cull,
        cull_layout,
        vertex_shader,
        fragment_shader,
    });
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

fn prepare_residency(
    extracted: Res<ExtractedChunks>,
    settings: Res<WorldRenderSettings>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<WorldPipelines>,
    mut residency: ResMut<WorldResidency>,
) {
    let epoch = extracted.epoch;
    residency
        .geometry_arena
        .get_or_insert_with(|| GeometryArena::new(&render_device, settings.geometry_budget_bytes));
    residency
        .svo_arena
        .get_or_insert_with(|| SvoArena::new(&render_device, DEFAULT_SVO_BUDGET_BYTES));
    residency
        .gpu_culling
        .get_or_insert_with(|| create_gpu_culling(&render_device, &pipeline_cache, &pipelines));
    for change in &extracted.changes {
        let ExtractedChunkChange::Transform {
            local_coordinate_id,
            world_from_local,
        } = change
        else {
            continue;
        };
        residency
            .local_transforms
            .insert(*local_coordinate_id, *world_from_local);
        let cull_descriptors = residency.gpu_culling.as_ref().unwrap().descriptors.clone();
        for (id, resident) in &mut residency.chunks {
            if id.local_coordinate_id != *local_coordinate_id {
                continue;
            }
            let updated = transformed_chunk_matrix(*world_from_local, id.coordinate);
            if resident.world_from_chunk != updated {
                resident.world_from_chunk = updated;
                write_chunk_transform(&render_queue, &cull_descriptors, resident);
            }
        }
    }

    for change in &extracted.changes {
        let ExtractedChunkChange::Remove(id) = change else {
            continue;
        };
        mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, *id);
        if let Some(mut previous) = residency.chunks.remove(id) {
            residency.slot_chunks[previous.gpu_slot as usize] = None;
            residency.free_gpu_slots.push(previous.gpu_slot);
            clear_gpu_draw(
                &render_queue,
                residency.gpu_culling.as_ref().unwrap(),
                previous.gpu_slot,
            );
            for geometry in previous.geometries.iter_mut().filter_map(Option::take) {
                residency.retired_geometry.push(RetiredGeometry {
                    retired_at_epoch: epoch,
                    geometry,
                });
            }
            residency.retired.push(RetiredChunk {
                retired_at_epoch: epoch,
                chunk: previous,
            });
        }
    }

    for change in &extracted.changes {
        let ExtractedChunkChange::Upsert(observed) = change else {
            continue;
        };
        residency.local_transforms.insert(
            observed.id.local_coordinate_id,
            local_matrix_from_chunk(observed.world_from_chunk, observed.id.coordinate),
        );
        let resident = residency.chunks.get(&observed.id);
        let update = resident_update(
            resident.map(|resident| (resident.revision, resident.world_from_chunk)),
            observed.revision,
            observed.world_from_chunk,
        );
        let cull_descriptors = residency.gpu_culling.as_ref().unwrap().descriptors.clone();
        if let Some(resident) = residency.chunks.get_mut(&observed.id) {
            match update {
                ResidentUpdate::Unchanged => continue,
                ResidentUpdate::Transform => {
                    resident.world_from_chunk = observed.world_from_chunk;
                    write_chunk_transform(&render_queue, &cull_descriptors, resident);
                    continue;
                }
                ResidentUpdate::Replace => {}
            }
        }

        let reused_gpu_slot = residency.chunks.contains_key(&observed.id);
        let gpu_slot = if let Some(resident) = residency.chunks.get(&observed.id) {
            resident.gpu_slot
        } else if let Some(slot) = residency.free_gpu_slots.pop() {
            slot
        } else {
            let slot = residency.next_gpu_slot;
            assert!(
                slot < MAX_GPU_CHUNK_DESCRIPTORS,
                "world GPU Chunk descriptor capacity exhausted"
            );
            residency.next_gpu_slot += 1;
            slot
        };
        let packed_svo = observed.svo.pack_with(|voxel| voxel.0);
        let svo = residency
            .svo_arena
            .as_mut()
            .unwrap()
            .upload(&render_device, &render_queue, &packed_svo.node_bytes())
            .expect("packed SVO arena budget must cover the loaded set");
        let mut replacement = resident_chunk(
            &render_device,
            observed,
            settings.material_seed,
            gpu_slot,
            svo,
        );
        residency.upload_count = residency.upload_count.wrapping_add(1);
        if let Some(mut previous) = residency.chunks.remove(&observed.id) {
            // Keep the last complete surface visible while the new revision waits
            // for geometry admission. It is deliberately key-mismatched and will
            // therefore never be mistaken for geometry of the new voxel content.
            for (replacement_lod, previous_lod) in replacement
                .geometries
                .iter_mut()
                .zip(previous.geometries.iter_mut())
            {
                *replacement_lod = previous_lod.take();
            }
            residency.retired.push(RetiredChunk {
                retired_at_epoch: epoch,
                chunk: previous,
            });
        }
        if !reused_gpu_slot {
            clear_gpu_draw(
                &render_queue,
                residency.gpu_culling.as_ref().unwrap(),
                replacement.gpu_slot,
            );
        }
        if residency.slot_chunks.len() <= gpu_slot as usize {
            residency.slot_chunks.resize(gpu_slot as usize + 1, None);
        }
        residency.slot_chunks[gpu_slot as usize] = Some(observed.id);
        residency.chunks.insert(observed.id, replacement);
        mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, observed.id);
    }

    let completed_epoch = residency.completed_submission_epoch.load(Ordering::Acquire);
    let retired_chunks = std::mem::take(&mut residency.retired);
    for retired in retired_chunks {
        if retired.retired_at_epoch <= completed_epoch {
            residency
                .svo_arena
                .as_mut()
                .unwrap()
                .free(retired.chunk.svo);
        } else {
            residency.retired.push(retired);
        }
    }
    let retired_geometry = std::mem::take(&mut residency.retired_geometry);
    let mut pending_retirement = Vec::with_capacity(retired_geometry.len());
    for retired in retired_geometry {
        if retired.retired_at_epoch <= completed_epoch {
            residency
                .geometry_arena
                .as_mut()
                .expect("geometry arena is initialized before retirement")
                .free(retired.geometry.allocation);
        } else {
            pending_retirement.push(retired);
        }
    }
    residency.retired_geometry = pending_retirement;

    // This callback covers every queue operation submitted before PrepareResources,
    // including the last frame that could reference resources retired at `epoch`.
    let completion = Arc::clone(&residency.completed_submission_epoch);
    render_queue.on_submitted_work_done(move || {
        completion.fetch_max(epoch, Ordering::Release);
    });
}

fn create_gpu_culling(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
) -> GpuCulling {
    let descriptor_size = u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 224;
    let draw_size =
        u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * u64::from(MAX_GPU_GEOMETRY_SEGMENTS) * 16;
    let descriptors =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world Chunk descriptors"),
            size: descriptor_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    let visible_draws =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world compact GPU-visible indirect draws"),
            size: draw_size,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT,
            mapped_at_creation: false,
        });
    let segment_counts =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world visible draw counts by geometry segment"),
            size: u64::from(MAX_GPU_GEOMETRY_SEGMENTS) * 4,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    let quad_counts =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world GPU quad counters"),
            size: u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 5 * 4,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
    let boundary_rows =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world Chunk boundary occupancy rows"),
            size: u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 96 * 4,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
    let build_slots =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world dirty GPU build slots"),
            size: u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 4,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    let bind_group = render_device.create_bind_group(
        Some("world GPU culling bind group"),
        &pipeline_cache.get_bind_group_layout(&pipelines.cull_layout),
        &BindGroupEntries::sequential((
            descriptors.as_entire_binding(),
            visible_draws.as_entire_binding(),
            segment_counts.as_entire_binding(),
        )),
    );
    GpuCulling {
        descriptors,
        visible_draws,
        segment_counts,
        quad_counts,
        boundary_rows,
        bind_group,
        render_segment_bind_groups: Vec::new(),
        build_bind_groups: Vec::new(),
        build_slots,
    }
}

fn write_cull_descriptor(
    render_queue: &RenderQueue,
    descriptors: &Buffer,
    chunk: &ResidentChunk,
    chunks: &HashMap<ChunkId, ResidentChunk>,
    id: ChunkId,
) {
    let draws = std::array::from_fn(|lod| {
        chunk.geometries[lod]
            .as_ref()
            .map_or(UVec4::ZERO, |geometry| {
                UVec4::new(
                    0,
                    u32::try_from(geometry.allocation.offset / GENERATED_QUAD_SIZE * 6)
                        .expect("geometry quad offset fits the indirect ABI"),
                    u32::try_from(geometry.allocation.segment)
                        .expect("geometry segment index fits the descriptor ABI"),
                    0,
                )
            })
    });
    let descriptor = GpuCullDescriptor {
        world_from_chunk: Mat4::from_cols_array(&chunk.world_from_chunk),
        chunk_coordinate: IVec4::new(
            chunk.coordinate[0] as i32,
            chunk.coordinate[1] as i32,
            chunk.coordinate[2] as i32,
            0,
        ),
        draws,
        metadata: UVec4::new(1, chunk.material_seed, 0, chunk.revision as u32),
        svo: UVec4::new(
            u32::try_from(chunk.svo.offset / 16).expect("SVO node offset fits GPU ABI"),
            u32::try_from(chunk.svo.size / 16).expect("SVO node count fits GPU ABI"),
            u32::try_from(chunk.svo.segment).expect("SVO segment fits GPU ABI"),
            1,
        ),
        neighbor_slots: [
            UVec4::from_array(std::array::from_fn(|face| {
                neighbor_id(id, CHUNK_NEIGHBOR_OFFSETS[face])
                    .and_then(|neighbor| chunks.get(&neighbor))
                    .map_or(u32::MAX, |neighbor| neighbor.gpu_slot)
            })),
            UVec4::new(
                neighbor_id(id, CHUNK_NEIGHBOR_OFFSETS[4])
                    .and_then(|neighbor| chunks.get(&neighbor))
                    .map_or(u32::MAX, |neighbor| neighbor.gpu_slot),
                neighbor_id(id, CHUNK_NEIGHBOR_OFFSETS[5])
                    .and_then(|neighbor| chunks.get(&neighbor))
                    .map_or(u32::MAX, |neighbor| neighbor.gpu_slot),
                u32::MAX,
                u32::MAX,
            ),
        ],
    };
    let mut bytes = Vec::with_capacity(224);
    for value in descriptor.world_from_chunk.to_cols_array() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in descriptor.chunk_coordinate.to_array() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for draw in descriptor.draws {
        for value in draw.to_array() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    for value in descriptor.metadata.to_array() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in descriptor.svo.to_array() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for neighbors in descriptor.neighbor_slots {
        for value in neighbors.to_array() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    render_queue.write_buffer(descriptors, u64::from(chunk.gpu_slot) * 224, &bytes);
}

fn write_chunk_transform(render_queue: &RenderQueue, descriptors: &Buffer, chunk: &ResidentChunk) {
    render_queue.write_buffer(
        descriptors,
        u64::from(chunk.gpu_slot) * 224,
        &world_matrix_bytes(chunk.world_from_chunk),
    );
}

fn clear_gpu_draw(render_queue: &RenderQueue, culling: &GpuCulling, slot: u32) {
    render_queue.write_buffer(&culling.descriptors, u64::from(slot) * 224, &[0; 224]);
}

fn resident_chunk(
    render_device: &RenderDevice,
    observed: &ExtractedChunk,
    material_seed: u32,
    gpu_slot: u32,
    svo: SvoAllocation,
) -> ResidentChunk {
    let _ = render_device;
    ResidentChunk {
        revision: observed.revision,
        gpu_slot,
        material_seed,
        coordinate: observed.id.coordinate,
        svo,
        geometries: std::array::from_fn(|_| None),
        world_from_chunk: observed.world_from_chunk,
    }
}

fn local_matrix_from_chunk(world_from_chunk: [f32; 16], coordinate: [i64; 3]) -> [f32; 16] {
    let inverse_translation = Mat4::from_translation(bevy::math::Vec3::new(
        coordinate[0] as f32 * -(CHUNK_EDGE as f32),
        coordinate[1] as f32 * -(CHUNK_EDGE as f32),
        coordinate[2] as f32 * -(CHUNK_EDGE as f32),
    ));
    (Mat4::from_cols_array(&world_from_chunk) * inverse_translation).to_cols_array()
}

fn transformed_chunk_matrix(world_from_local: [f32; 16], coordinate: [i64; 3]) -> [f32; 16] {
    let translation = Mat4::from_translation(bevy::math::Vec3::new(
        coordinate[0] as f32 * CHUNK_EDGE as f32,
        coordinate[1] as f32 * CHUNK_EDGE as f32,
        coordinate[2] as f32 * CHUNK_EDGE as f32,
    ));
    (Mat4::from_cols_array(&world_from_local) * translation).to_cols_array()
}

fn world_matrix_bytes(matrix: [f32; 16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    for value in matrix {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
fn indirect_bytes() -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
    bytes
}

fn world_compute(
    view: ViewQuery<(&ExtractedView, &ExtractedCamera, &ViewUniformOffset)>,
    view_bind_group: Res<WorldViewBindGroup>,
    extracted: Res<ExtractedChunks>,
    pipelines: Res<WorldPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    _settings: Res<WorldRenderSettings>,
    mut residency: ResMut<WorldResidency>,
    mut context: RenderContext,
) {
    if residency.chunks.is_empty() {
        return;
    }
    let started = Instant::now();
    let (_, _, view_offset) = view.into_inner();
    let (
        Some(boundary_pipeline),
        Some(reset_pipeline),
        Some(mesh_pipeline),
        Some(finalize_pipeline),
    ) = (
        pipeline_cache.get_compute_pipeline(pipelines.prepare_boundary),
        pipeline_cache.get_compute_pipeline(pipelines.reset_geometry),
        pipeline_cache.get_compute_pipeline(pipelines.compute),
        pipeline_cache.get_compute_pipeline(pipelines.finalize_builds),
    )
    else {
        return;
    };

    let dirty = std::mem::take(&mut residency.geometry_dirty);
    let mut prepared_build_slots = Vec::with_capacity(dirty.len());
    for id in dirty {
        if !residency.chunks.contains_key(&id) {
            continue;
        }
        let mut complete = true;
        for lod in 0..=MAX_CHUNK_LOD {
            let index = lod as usize;
            let bytes = geometry_bytes_for_quads(maximum_quads(lod));
            if residency.chunks[&id].geometries[index].is_none() {
                let Some(allocation) = residency
                    .geometry_arena
                    .as_mut()
                    .unwrap()
                    .allocate(&render_device, bytes)
                else {
                    complete = false;
                    residency.stats.geometry_budget_rejections += 1;
                    break;
                };
                let revision = residency.chunks[&id].revision;
                let boundary_signature = chunk_boundary_signature(id, &residency.chunks, lod);
                residency.chunks.get_mut(&id).unwrap().geometries[index] = Some(ResidentGeometry {
                    key: GeometryKey {
                        source_revision: revision,
                        lod,
                        boundary_signature,
                    },
                    allocated_bytes: bytes,
                    allocation,
                });
                residency.stats.geometry_builds += 1;
            } else {
                let revision = residency.chunks[&id].revision;
                let signature = chunk_boundary_signature(id, &residency.chunks, lod);
                residency.chunks.get_mut(&id).unwrap().geometries[index]
                    .as_mut()
                    .unwrap()
                    .key = GeometryKey {
                    source_revision: revision,
                    lod,
                    boundary_signature: signature,
                };
            }
        }
        if complete {
            prepared_build_slots.push(residency.chunks[&id].gpu_slot);
            write_cull_descriptor(
                &render_queue,
                &residency.gpu_culling.as_ref().unwrap().descriptors,
                &residency.chunks[&id],
                &residency.chunks,
                id,
            );
        } else {
            residency.geometry_dirty.insert(id);
        }
    }

    {
        let WorldResidency {
            geometry_arena,
            svo_arena,
            gpu_culling,
            ..
        } = &mut *residency;
        sync_render_segment_bind_groups(
            &render_device,
            &pipeline_cache,
            &pipelines,
            geometry_arena.as_ref().unwrap(),
            gpu_culling.as_mut().unwrap(),
        );
        sync_build_bind_groups(
            &render_device,
            &pipeline_cache,
            &pipelines,
            svo_arena.as_ref().unwrap(),
            geometry_arena.as_ref().unwrap(),
            gpu_culling.as_mut().unwrap(),
        );
    }

    let descriptor_count = residency.next_gpu_slot;
    let build_count = prepared_build_slots.len() as u32;
    if build_count > 0 {
        let padded_count = build_count.div_ceil(64) * 64;
        prepared_build_slots.resize(padded_count as usize, u32::MAX);
        let bytes = prepared_build_slots
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        render_queue.write_buffer(
            &residency.gpu_culling.as_ref().unwrap().build_slots,
            0,
            &bytes,
        );
    }
    let svo_segments = residency.svo_arena.as_ref().unwrap().segment_count();
    let geometry_segments = residency.geometry_arena.as_ref().unwrap().segment_count();
    if build_count > 0 && geometry_segments > 0 {
        {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world GPU boundary summary build"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(boundary_pipeline);
            for svo in 0..svo_segments {
                pass.set_bind_group(
                    0,
                    &residency.gpu_culling.as_ref().unwrap().build_bind_groups[svo][0],
                    &[],
                );
                pass.dispatch_workgroups(96, build_count, 1);
            }
        }
        {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world GPU geometry reset"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(reset_pipeline);
            for svo in 0..svo_segments {
                for geometry in 0..geometry_segments {
                    pass.set_bind_group(
                        0,
                        &residency.gpu_culling.as_ref().unwrap().build_bind_groups[svo][geometry],
                        &[],
                    );
                    pass.dispatch_workgroups(build_count.div_ceil(64), 1, 1);
                }
            }
        }
        {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world GPU packed-SVO greedy meshing"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(mesh_pipeline);
            for svo in 0..svo_segments {
                for geometry in 0..geometry_segments {
                    pass.set_bind_group(
                        0,
                        &residency.gpu_culling.as_ref().unwrap().build_bind_groups[svo][geometry],
                        &[],
                    );
                    let (x, y, z) = geometry_build_dispatch(build_count);
                    pass.dispatch_workgroups(x, y, z);
                }
            }
        }
        {
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("world GPU geometry commit"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(finalize_pipeline);
            for svo in 0..svo_segments {
                pass.set_bind_group(
                    0,
                    &residency.gpu_culling.as_ref().unwrap().build_bind_groups[svo][0],
                    &[],
                );
                pass.dispatch_workgroups(build_count.div_ceil(64), 1, 1);
            }
        }
    }

    render_queue.write_buffer(
        &residency.gpu_culling.as_ref().unwrap().segment_counts,
        0,
        &[0; MAX_GPU_GEOMETRY_SEGMENTS as usize * 4],
    );
    if let Some(cull_pipeline) = pipeline_cache.get_compute_pipeline(pipelines.cull) {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("world parallel GPU frustum culling and LOD selection"),
                timestamp_writes: None,
            });
        pass.set_pipeline(cull_pipeline);
        pass.set_bind_group(0, &view_bind_group.0, &[view_offset.offset]);
        pass.set_bind_group(1, &residency.gpu_culling.as_ref().unwrap().bind_group, &[]);
        pass.dispatch_workgroups(descriptor_count.div_ceil(64), 1, 1);
    }

    residency.stats.compute_cpu_micros = residency
        .stats
        .compute_cpu_micros
        .saturating_add(started.elapsed().as_micros() as u64);
    if extracted
        .epoch
        .wrapping_sub(residency.stats.last_report_epoch)
        >= 120
    {
        bevy::log::debug!(
            "world render: resident={}, drawable={}, builds={}, budget_rejections={}, active_mib={}, compute_cpu_us={} (120-frame window)",
            residency.chunks.len(),
            residency
                .chunks
                .values()
                .filter(|chunk| chunk.geometries.iter().any(Option::is_some))
                .count(),
            residency.stats.geometry_builds,
            residency.stats.geometry_budget_rejections,
            geometry_active_bytes(&residency.chunks) / (1024 * 1024),
            residency.stats.compute_cpu_micros,
        );
        residency.stats = WorldRenderStats {
            last_report_epoch: extracted.epoch,
            ..Default::default()
        };
    }
}

fn geometry_active_bytes(chunks: &HashMap<ChunkId, ResidentChunk>) -> u64 {
    chunks
        .values()
        .flat_map(|chunk| chunk.geometries.iter().filter_map(Option::as_ref))
        .map(|geometry| geometry.allocated_bytes)
        .sum()
}

fn sync_render_segment_bind_groups(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
    arena: &GeometryArena,
    culling: &mut GpuCulling,
) {
    assert!(
        arena.segment_count() <= MAX_GPU_GEOMETRY_SEGMENTS as usize,
        "geometry arena exceeded the GPU batch segment ABI"
    );
    while culling.render_segment_bind_groups.len() < arena.segment_count() {
        let segment = culling.render_segment_bind_groups.len();
        culling
            .render_segment_bind_groups
            .push(render_device.create_bind_group(
                Some("world batched geometry segment"),
                &pipeline_cache.get_bind_group_layout(&pipelines.render_layout),
                &BindGroupEntries::sequential((
                    arena.segment_buffer(segment).as_entire_binding(),
                    culling.descriptors.as_entire_binding(),
                )),
            ));
    }
}

fn sync_build_bind_groups(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
    svo_arena: &SvoArena,
    geometry_arena: &GeometryArena,
    culling: &mut GpuCulling,
) {
    let dimensions_match = culling.build_bind_groups.len() == svo_arena.segment_count()
        && culling
            .build_bind_groups
            .iter()
            .all(|groups| groups.len() == geometry_arena.segment_count());
    if dimensions_match {
        return;
    }
    culling.build_bind_groups.clear();
    let layout = pipeline_cache.get_bind_group_layout(&pipelines.compute_layout);
    for svo_segment in 0..svo_arena.segment_count() {
        let mut row = Vec::with_capacity(geometry_arena.segment_count());
        for geometry_segment in 0..geometry_arena.segment_count() {
            let pass = [
                svo_segment as u32,
                geometry_segment as u32,
                MAX_GPU_CHUNK_DESCRIPTORS,
                0,
            ];
            let bytes = pass
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>();
            let uniform = render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("world GPU build pass parameters"),
                contents: &bytes,
                usage: BufferUsages::UNIFORM,
            });
            row.push(
                render_device.create_bind_group(
                    Some("world GPU batched geometry build"),
                    &layout,
                    &BindGroupEntries::sequential((
                        svo_arena.segment_buffer(svo_segment).as_entire_binding(),
                        geometry_arena
                            .segment_buffer(geometry_segment)
                            .as_entire_binding(),
                        culling.descriptors.as_entire_binding(),
                        culling.quad_counts.as_entire_binding(),
                        culling.boundary_rows.as_entire_binding(),
                        uniform.as_entire_binding(),
                        culling.build_slots.as_entire_binding(),
                    )),
                ),
            );
        }
        culling.build_bind_groups.push(row);
    }
}

const CHUNK_NEIGHBOR_OFFSETS: [[i64; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

fn neighbor_id(id: ChunkId, offset: [i64; 3]) -> Option<ChunkId> {
    Some(ChunkId {
        local_coordinate_id: id.local_coordinate_id,
        coordinate: [
            id.coordinate[0].checked_add(offset[0])?,
            id.coordinate[1].checked_add(offset[1])?,
            id.coordinate[2].checked_add(offset[2])?,
        ],
    })
}

fn mark_chunk_and_neighbors_dirty(dirty: &mut HashSet<ChunkId>, id: ChunkId) {
    dirty.insert(id);
    for offset in CHUNK_NEIGHBOR_OFFSETS {
        if let Some(neighbor) = neighbor_id(id, offset) {
            dirty.insert(neighbor);
        }
    }
}

fn chunk_boundary_signature(
    id: ChunkId,
    chunks: &HashMap<ChunkId, ResidentChunk>,
    _lod: u8,
) -> u64 {
    boundary_signature_from_neighbors(id, |neighbor| {
        chunks.get(&neighbor).map(|chunk| (chunk.revision, 0))
    })
}

fn boundary_signature_from_neighbors(
    id: ChunkId,
    mut state: impl FnMut(ChunkId) -> Option<(u64, u8)>,
) -> u64 {
    CHUNK_NEIGHBOR_OFFSETS
        .into_iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |signature, offset| {
            let state =
                neighbor_id(id, offset)
                    .and_then(&mut state)
                    .map_or(0, |(revision, lod)| {
                        revision
                            .wrapping_add(1)
                            .rotate_left(8)
                            .wrapping_add(u64::from(lod) + 1)
                    });
            (signature ^ state).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn geometry_build_dispatch(build_count: u32) -> (u32, u32, u32) {
    (5 * FACE_PLANE_COUNT, build_count, 1)
}

fn maximum_quads(lod: u8) -> u32 {
    let grid = u32::from(CHUNK_EDGE >> lod);
    // A checkerboard occupancy maximizes unmerged interior faces. Coarse LODs
    // additionally retain six finest-resolution boundary planes so adjacent
    // independently selected LODs always share the same edge tessellation.
    3 * grid * grid * grid + 6 * (CHUNK_EDGE as u32 * CHUNK_EDGE as u32 / 2)
}

fn geometry_bytes_for_quads(quad_count: u32) -> u64 {
    u64::from(quad_count.max(1)) * GENERATED_QUAD_SIZE
}

#[cfg(test)]
fn aabb_intersects_clip(clip_from_chunk: Mat4) -> bool {
    let edge = CHUNK_EDGE as f32;
    let mut outside = [true; 6];
    for z in [0.0, edge] {
        for y in [0.0, edge] {
            for x in [0.0, edge] {
                let point = clip_from_chunk * Vec4::new(x, y, z, 1.0);
                outside[0] &= point.x < -point.w;
                outside[1] &= point.x > point.w;
                outside[2] &= point.y < -point.w;
                outside[3] &= point.y > point.w;
                outside[4] &= point.z < 0.0;
                outside[5] &= point.z > point.w;
            }
        }
    }
    !outside.into_iter().any(|plane| plane)
}

#[cfg(test)]
fn chunk_distance_squared(
    world_from_chunk: [f32; 16],
    camera_position: bevy::prelude::Vec3,
) -> f32 {
    let transform = Mat4::from_cols_array(&world_from_chunk);
    let local_camera = transform.inverse().transform_point3(camera_position);
    let nearest_local = local_camera.clamp(
        bevy::prelude::Vec3::ZERO,
        bevy::prelude::Vec3::splat(CHUNK_EDGE as f32),
    );
    transform
        .transform_point3(nearest_local)
        .distance_squared(camera_position)
}

fn world_primitive_state() -> PrimitiveState {
    PrimitiveState {
        topology: PrimitiveTopology::TriangleList,
        strip_index_format: None,
        front_face: FrontFace::Ccw,
        cull_mode: Some(Face::Back),
        unclipped_depth: false,
        polygon_mode: PolygonMode::Fill,
        conservative: false,
    }
}

#[derive(Resource, Default)]
struct WorldViewPipelines(HashMap<(TextureFormat, u32), CachedRenderPipelineId>);

#[derive(Component)]
struct WorldViewPipeline(CachedRenderPipelineId);

fn prepare_view_pipelines(
    mut commands: Commands,
    views: Query<(Entity, &ViewTarget, &Msaa)>,
    pipelines: Res<WorldPipelines>,
    pipeline_cache: Res<PipelineCache>,
    mut view_pipelines: ResMut<WorldViewPipelines>,
) {
    for (entity, target, msaa) in &views {
        let key = (target.main_texture_format(), msaa.samples());
        let id = *view_pipelines.0.entry(key).or_insert_with(|| {
            pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(Cow::Borrowed("world generated triangles")),
                layout: vec![
                    pipelines.view_layout.clone(),
                    pipelines.render_layout.clone(),
                ],
                vertex: VertexState {
                    shader: pipelines.vertex_shader.clone(),
                    entry_point: Some(Cow::Borrowed("vertex")),
                    buffers: Vec::new(),
                    ..Default::default()
                },
                primitive: world_primitive_state(),
                depth_stencil: Some(DepthStencilState {
                    format: TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(CompareFunction::Greater),
                    stencil: StencilState::default(),
                    bias: Default::default(),
                }),
                multisample: MultisampleState {
                    count: msaa.samples(),
                    ..Default::default()
                },
                fragment: Some(FragmentState {
                    shader: pipelines.fragment_shader.clone(),
                    entry_point: Some(Cow::Borrowed("fragment")),
                    targets: vec![Some(ColorTargetState {
                        format: target.main_texture_format(),
                        blend: Some(BlendState::REPLACE),
                        write_mask: ColorWrites::ALL,
                    })],
                    ..Default::default()
                }),
                ..Default::default()
            })
        });
        commands.entity(entity).insert(WorldViewPipeline(id));
    }
}

#[derive(Resource)]
struct WorldViewBindGroup(BindGroup);

fn prepare_view_bind_group(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    view_uniforms: Res<ViewUniforms>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<WorldPipelines>,
) {
    let Some(binding) = view_uniforms.uniforms.binding() else {
        return;
    };
    commands.insert_resource(WorldViewBindGroup(render_device.create_bind_group(
        Some("world view bind group"),
        &pipeline_cache.get_bind_group_layout(&pipelines.view_layout),
        &BindGroupEntries::single(binding),
    )));
}

fn world_draw(
    view: ViewQuery<(
        &ViewTarget,
        &ViewDepthTexture,
        &ViewUniformOffset,
        &WorldViewPipeline,
    )>,
    view_bind_group: Res<WorldViewBindGroup>,
    pipelines: Res<PipelineCache>,
    residency: Res<WorldResidency>,
    mut context: RenderContext,
) {
    if residency.chunks.is_empty() {
        return;
    }
    let (target, depth, view_offset, pipeline_id) = view.into_inner();
    let Some(pipeline) = pipelines.get_render_pipeline(pipeline_id.0) else {
        return;
    };
    let color_attachments = [Some(target.get_color_attachment())];
    let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("world generated triangles"),
        color_attachments: &color_attachments,
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, &view_bind_group.0, &[view_offset.offset]);
    let culling = residency
        .gpu_culling
        .as_ref()
        .expect("GPU culling exists while resident Chunks are drawable");
    for (segment, bind_group) in culling.render_segment_bind_groups.iter().enumerate() {
        pass.set_bind_group(1, bind_group, &[]);
        pass.wgpu_pass().multi_draw_indirect_count(
            &culling.visible_draws,
            segment as u64 * u64::from(MAX_GPU_CHUNK_DESCRIPTORS) * 16,
            &culling.segment_counts,
            segment as u64 * 4,
            MAX_GPU_CHUNK_DESCRIPTORS,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_geometry_dispatch_scales_with_changes_not_resident_chunks() {
        assert_eq!(geometry_build_dispatch(0), (480, 0, 1));
        assert_eq!(geometry_build_dispatch(7), (480, 7, 1));
        // A 4096-Chunk resident set with seven changed descriptors still emits
        // only seven Y workgroups; the old implementation emitted 4096.
        assert_ne!(geometry_build_dispatch(7).1, 4096);
    }

    #[test]
    fn fixed_quad_capacities_cover_checkerboard_and_boundary_planes() {
        assert_eq!(maximum_quads(0), 13_056);
        assert_eq!(maximum_quads(1), 2_304);
        assert_eq!(maximum_quads(2), 960);
        assert_eq!(maximum_quads(3), 792);
        assert_eq!(maximum_quads(4), 771);
        assert_eq!((0..=MAX_CHUNK_LOD).map(maximum_quads).sum::<u32>(), 17_883);
    }

    #[test]
    fn gpu_chunk_descriptor_abi_has_expected_stride() {
        assert_eq!(GpuCullDescriptor::min_size().get(), 224);
        assert_eq!(GpuBuildPass::min_size().get(), 16);
    }

    #[test]
    fn material_seed_has_the_documented_default() {
        let settings = WorldRenderSettings::default();
        assert_eq!(settings.material_seed, 0xDEAD_BEEF);
        assert_eq!(settings.geometry_budget_bytes, 1024 * 1024 * 1024);
    }

    #[test]
    fn indirect_draw_starts_with_one_instance_and_no_vertices() {
        let bytes = indirect_bytes();
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
    }

    #[test]
    fn clip_culling_rejects_chunks_fully_outside_one_plane() {
        assert!(aabb_intersects_clip(Mat4::IDENTITY));
        assert!(!aabb_intersects_clip(Mat4::from_translation(
            bevy::prelude::Vec3::new(100.0, 0.0, 0.0)
        )));
    }

    #[test]
    fn culling_distance_uses_all_three_dimensions_of_the_chunk_aabb() {
        let transform = Mat4::IDENTITY.to_cols_array();
        assert_eq!(
            chunk_distance_squared(transform, bevy::prelude::Vec3::new(8.0, 8.0, 8.0)),
            0.0
        );
        assert_eq!(
            chunk_distance_squared(transform, bevy::prelude::Vec3::new(8.0, 20.0, 8.0)),
            16.0
        );
        assert_eq!(
            chunk_distance_squared(transform, bevy::prelude::Vec3::new(20.0, 20.0, 8.0)),
            32.0
        );
    }

    #[test]
    fn world_triangles_cull_back_faces() {
        assert_eq!(world_primitive_state().cull_mode, Some(Face::Back));
    }

    #[test]
    fn residency_reuses_equal_revisions_and_replaces_changed_revisions() {
        let transform = Mat4::IDENTITY.to_cols_array();
        assert_eq!(
            resident_update(Some((4, transform)), 4, transform),
            ResidentUpdate::Unchanged
        );
        assert_eq!(
            resident_update(
                Some((4, transform)),
                4,
                Mat4::from_translation(bevy::prelude::Vec3::X).to_cols_array(),
            ),
            ResidentUpdate::Transform
        );
        assert_eq!(
            resident_update(Some((4, transform)), 5, transform),
            ResidentUpdate::Replace
        );
        assert_eq!(resident_update(None, 1, transform), ResidentUpdate::Replace);
    }

    #[test]
    fn chunk_neighbors_continue_into_negative_coordinates() {
        let first = ChunkId {
            local_coordinate_id: roundo_local_coordinate::LocalCoordinateId(1),
            coordinate: [0, 0, 0],
        };
        assert_eq!(
            neighbor_id(first, [-1, 0, 0]).unwrap().coordinate,
            [-1, 0, 0]
        );
    }

    #[test]
    fn neighbor_revision_changes_invalidate_boundary_geometry() {
        let center = ChunkId {
            local_coordinate_id: roundo_local_coordinate::LocalCoordinateId(1),
            coordinate: [0, 0, 0],
        };
        let neighbor = neighbor_id(center, [1, 0, 0]).unwrap();
        let absent = boundary_signature_from_neighbors(center, |_| None);
        let revision_four =
            boundary_signature_from_neighbors(center, |id| (id == neighbor).then_some((4, 2)));
        let revision_five =
            boundary_signature_from_neighbors(center, |id| (id == neighbor).then_some((5, 2)));
        let different_lod =
            boundary_signature_from_neighbors(center, |id| (id == neighbor).then_some((5, 1)));
        assert_ne!(absent, revision_four);
        assert_ne!(revision_four, revision_five);
        assert_ne!(revision_five, different_lod);
    }

    #[test]
    fn meshing_uses_cross_chunk_neighbors() {
        let shader = include_str!("shaders/world_greedy_emit.wgsl");
        assert!(
            shader.contains("neighbor_slots") && shader.contains("boundary_rows"),
            "meshing shader has no cross-chunk neighbor input"
        );
    }

    #[test]
    fn lod_meshing_queries_the_packed_svo_on_gpu() {
        let shader = include_str!("shaders/world_greedy_emit.wgsl");
        assert!(shader.contains("PackedSvoNode"));
        assert!(shader.contains("node.reserved & 1u"));
        assert!(shader.contains("return vec3(16u >> lod)"));
        assert!(!shader.contains("OCCUPANCY_MIP_OFFSETS"));
        assert!(!shader.contains("horizontal"));
    }
}
