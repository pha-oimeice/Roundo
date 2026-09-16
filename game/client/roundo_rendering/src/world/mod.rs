//! GPU-driven voxel-chunk extraction, residency, LOD, geometry generation, and drawing.
//!
//! Main-world chunks are copied into render-world snapshots. Geometry replacement
//! is incremental and budgeted: an older complete mesh remains drawable until a
//! replacement has been generated, read back, allocated, and made resident.

mod arena;
mod lod;

use arena::{GeometryAllocation, GeometryArena};

use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        core_3d::main_opaque_pass_3d,
        schedule::{Core3d, Core3dSystems},
    },
    prelude::{
        App, Commands, Component, DetectChanges, Entity, GlobalTransform, IVec4,
        IntoScheduleConfigs, Mat4, Query, Ref, Res, ResMut, Resource, UVec4, Vec4,
    },
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BindingResource, BlendState, Buffer, BufferBinding, BufferInitDescriptor, BufferSize,
            BufferUsages, CachedComputePipelineId, CachedRenderPipelineId, ColorTargetState,
            ColorWrites, CompareFunction, ComputePassDescriptor, ComputePipelineDescriptor,
            DepthStencilState, Face, FragmentState, FrontFace, MapMode, MultisampleState,
            PipelineCache, PolygonMode, PrimitiveState, PrimitiveTopology, RenderPassDescriptor,
            RenderPipelineDescriptor, ShaderStages, ShaderType, StencilState, StoreOp,
            TextureFormat, VertexState,
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
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Instant,
};

/// Default deterministic seed used by the voxel material shader.
pub const DEFAULT_WORLD_MATERIAL_SEED: u32 = 0xDEAD_BEEF;
const DEFAULT_GEOMETRY_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const CHUNK_EDGE: u32 = CHUNK_EDGE_LENGTH as u32;
const FACE_PLANE_COUNT: u32 = 6 * CHUNK_EDGE;
const MAX_CHUNK_LOD: u8 = CHUNK_EDGE.ilog2() as u8;
// The GPU ABI and dispatch topology below intentionally model one fixed 16³ cube.
// Fail compilation instead of silently drifting into a 16²×height representation.
const _: () = assert!(CHUNK_EDGE == 16 && MAX_CHUNK_LOD == 4);
const GENERATED_VERTEX_SIZE: u64 = 16;
const DEFAULT_GEOMETRY_BUILD_BUDGET_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_MAX_GEOMETRY_BUILDS_PER_FRAME: u32 = 32;
// Bit offsets for the complete 16³ → 8³ → 4³ → 2³ → 1³ occupancy pyramid.
const OCCUPANCY_MIP_LEVEL_OFFSETS: [u32; 5] = [0, 4096, 4608, 4672, 4680];
const OCCUPANCY_MIP_CELL_COUNT: u32 = 4681;
const OCCUPANCY_MIP_WORD_COUNT: usize = OCCUPANCY_MIP_CELL_COUNT.div_ceil(u32::BITS) as usize;

/// Render-world budgets and deterministic material input.
///
/// Callers should configure this resource before the renderer initializes.
/// Runtime changes do not retroactively rebuild every resident chunk, and the
/// geometry arena retains the total-reservation limit used when it is first
/// created. Zero budgets are valid and can prevent new geometry from becoming
/// resident without evicting an older complete mesh.
#[derive(Clone, Copy, Debug, ExtractResource, Resource)]
pub struct WorldRenderSettings {
    /// Seed mixed into deterministic material/color selection for newly prepared chunks.
    pub material_seed: u32,
    /// Maximum total bytes reserved by retained geometry-arena segments.
    pub geometry_budget_bytes: u64,
    /// Maximum generated-geometry bytes admitted during one render frame.
    pub geometry_build_budget_bytes: u64,
    /// Maximum geometry count/emit dispatches admitted in one render frame.
    pub max_geometry_builds_per_frame: u32,
}

impl Default for WorldRenderSettings {
    fn default() -> Self {
        Self {
            material_seed: DEFAULT_WORLD_MATERIAL_SEED,
            geometry_budget_bytes: DEFAULT_GEOMETRY_BUDGET_BYTES,
            geometry_build_budget_bytes: DEFAULT_GEOMETRY_BUILD_BUDGET_BYTES,
            max_geometry_builds_per_frame: DEFAULT_MAX_GEOMETRY_BUILDS_PER_FRAME,
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
        embedded_asset!(app, "shaders/world_common.wgsl");
        embedded_asset!(app, "shaders/world_cull_lod.wgsl");
        embedded_asset!(app, "shaders/world_face_mask.wgsl");
        embedded_asset!(app, "shaders/world_greedy_count.wgsl");
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
    lod_contexts: HashMap<LocalCoordinateId, lod::LodContext>,
    geometry_dirty: HashSet<ChunkId>,
    build_needed: HashSet<ChunkId>,
    geometry_arena: Option<GeometryArena>,
    retired: Vec<RetiredChunk>,
    retired_geometry: Vec<RetiredGeometry>,
    upload_count: u64,
    completed_submission_epoch: Arc<AtomicU64>,
    stats: WorldRenderStats,
    pending_count: Option<PendingGeometryCount>,
}

#[derive(Default)]
struct WorldRenderStats {
    last_report_epoch: u64,
    geometry_builds: u64,
    geometry_budget_rejections: u64,
    active_evictions: u64,
    compute_cpu_micros: u64,
}

struct ResidentChunk {
    revision: u64,
    occupancy_mip: Buffer,
    uniform: Buffer,
    geometry: Option<ResidentGeometry>,
    desired_lod: u8,
    desired_key: Option<GeometryKey>,
    pending_key: Option<GeometryKey>,
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
    indirect: Buffer,
    compute_bind_group: BindGroup,
    render_bind_group: BindGroup,
}

struct PendingGeometryCount {
    entries: Vec<(ChunkId, GeometryKey)>,
    readback: Buffer,
    stride: u64,
    status: Arc<AtomicU8>,
}

struct RetiredChunk {
    retired_at_epoch: u64,
    _chunk: ResidentChunk,
}

struct RetiredGeometry {
    retired_at_epoch: u64,
    geometry: ResidentGeometry,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuGeneratedVertex {
    position_and_face: Vec4,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuDrawIndirect {
    words: UVec4,
}

#[derive(Clone, Copy, ShaderType)]
struct WorldChunkUniform {
    world_from_chunk: Mat4,
    svo: UVec4,
    chunk_coordinate: IVec4,
    neighbor_svo: [UVec4; 6],
}

#[derive(Resource)]
struct WorldPipelines {
    compute_layout: BindGroupLayoutDescriptor,
    view_layout: BindGroupLayoutDescriptor,
    render_layout: BindGroupLayoutDescriptor,
    compute: CachedComputePipelineId,
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
                storage_buffer_read_only::<u32>(false),
                storage_buffer::<GpuGeneratedVertex>(false),
                storage_buffer::<GpuDrawIndirect>(false),
                uniform_buffer::<WorldChunkUniform>(false),
                storage_buffer_read_only::<u32>(false),
                storage_buffer_read_only::<u32>(false),
                storage_buffer_read_only::<u32>(false),
                storage_buffer_read_only::<u32>(false),
                storage_buffer_read_only::<u32>(false),
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
                storage_buffer_read_only::<GpuGeneratedVertex>(false),
                uniform_buffer::<WorldChunkUniform>(false),
            ),
        ),
    );
    let compute_shader =
        load_embedded_asset!(asset_server.as_ref(), "shaders/world_greedy_emit.wgsl");
    let vertex_shader = load_embedded_asset!(asset_server.as_ref(), "shaders/world_vertex.wgsl");
    let fragment_shader =
        load_embedded_asset!(asset_server.as_ref(), "shaders/world_fragment.wgsl");
    let compute = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some(Cow::Borrowed("world SVO greedy meshing")),
        layout: vec![compute_layout.clone()],
        shader: compute_shader,
        entry_point: Some(Cow::Borrowed("mesh_chunk")),
        ..Default::default()
    });
    commands.insert_resource(WorldPipelines {
        compute_layout,
        view_layout,
        render_layout,
        compute,
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
        for (id, resident) in &mut residency.chunks {
            if id.local_coordinate_id != *local_coordinate_id {
                continue;
            }
            let updated = transformed_chunk_matrix(*world_from_local, id.coordinate);
            if resident.world_from_chunk != updated {
                resident.world_from_chunk = updated;
                render_queue.write_buffer(&resident.uniform, 0, &world_matrix_bytes(updated));
            }
        }
    }

    for change in &extracted.changes {
        let ExtractedChunkChange::Remove(id) = change else {
            continue;
        };
        mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, *id);
        if let Some(mut previous) = residency.chunks.remove(id) {
            if let Some(geometry) = previous.geometry.take() {
                residency.retired_geometry.push(RetiredGeometry {
                    retired_at_epoch: epoch,
                    geometry,
                });
            }
            residency.retired.push(RetiredChunk {
                retired_at_epoch: epoch,
                _chunk: previous,
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
        if let Some(resident) = residency.chunks.get_mut(&observed.id) {
            match update {
                ResidentUpdate::Unchanged => continue,
                ResidentUpdate::Transform => {
                    resident.world_from_chunk = observed.world_from_chunk;
                    let matrix = world_matrix_bytes(observed.world_from_chunk);
                    render_queue.write_buffer(&resident.uniform, 0, &matrix);
                    continue;
                }
                ResidentUpdate::Replace => {}
            }
        }

        let mut replacement = resident_chunk(&render_device, observed, settings.material_seed);
        residency.upload_count = residency.upload_count.wrapping_add(1);
        if let Some(mut previous) = residency.chunks.remove(&observed.id) {
            // Keep the last complete surface visible while the new revision waits
            // for geometry admission. It is deliberately key-mismatched and will
            // therefore never be mistaken for geometry of the new voxel content.
            if let Some(mut fallback) = previous.geometry.take() {
                let geometry_buffer = residency
                    .geometry_arena
                    .as_ref()
                    .expect("geometry arena is initialized before residency replacement")
                    .buffer(fallback.allocation)
                    .clone();
                fallback.render_bind_group = create_render_bind_group(
                    &render_device,
                    &pipeline_cache,
                    &pipelines,
                    &geometry_buffer,
                    fallback.allocation,
                    &replacement.uniform,
                );
                replacement.geometry = Some(fallback);
            }
            residency.retired.push(RetiredChunk {
                retired_at_epoch: epoch,
                _chunk: previous,
            });
        }
        residency.chunks.insert(observed.id, replacement);
        mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, observed.id);
    }

    let completed_epoch = residency.completed_submission_epoch.load(Ordering::Acquire);
    residency
        .retired
        .retain(|retired| retired.retired_at_epoch > completed_epoch);
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

fn resident_chunk(
    render_device: &RenderDevice,
    observed: &ExtractedChunk,
    material_seed: u32,
) -> ResidentChunk {
    let occupancy_words = build_occupancy_mip(|x, y, z| {
        observed
            .svo
            .value_at_coordinates([x, y, z])
            .is_some_and(|voxel| voxel.0 != 0)
    });
    let occupancy_bytes = occupancy_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect::<Vec<_>>();
    let occupancy_mip = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world chunk 16-cubed occupancy mip"),
        contents: &occupancy_bytes,
        usage: BufferUsages::STORAGE,
    });
    let uniform_bytes = chunk_uniform_bytes(observed, material_seed);
    let uniform = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world chunk uniform"),
        contents: &uniform_bytes,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
    });
    ResidentChunk {
        revision: observed.revision,
        occupancy_mip,
        uniform,
        geometry: None,
        desired_lod: 0,
        desired_key: None,
        pending_key: None,
        world_from_chunk: observed.world_from_chunk,
    }
}

fn chunk_uniform_bytes(observed: &ExtractedChunk, material_seed: u32) -> Vec<u8> {
    let mut bytes = world_matrix_bytes(observed.world_from_chunk);
    bytes.reserve(32);
    let metadata = [0, u32::from(MAX_CHUNK_LOD), 0, material_seed];
    for value in metadata {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in observed.id.coordinate {
        bytes.extend_from_slice(&(value as i32).to_le_bytes());
    }
    bytes.extend_from_slice(&0_i32.to_le_bytes());
    for _ in 0..6 {
        for value in [0_u32; 4] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
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

fn occupancy_mip_index(lod: u8, x: u32, y: u32, z: u32) -> u32 {
    let edge = CHUNK_EDGE >> lod;
    OCCUPANCY_MIP_LEVEL_OFFSETS[usize::from(lod)] + (z * edge + y) * edge + x
}

fn mip_bit(words: &[u32], index: u32) -> bool {
    words[index as usize / u32::BITS as usize] & (1 << (index % u32::BITS)) != 0
}

fn set_mip_bit(words: &mut [u32], index: u32) {
    words[index as usize / u32::BITS as usize] |= 1 << (index % u32::BITS);
}

fn build_occupancy_mip(mut occupied: impl FnMut(u32, u32, u32) -> bool) -> Vec<u32> {
    let mut words = vec![0_u32; OCCUPANCY_MIP_WORD_COUNT];
    for z in 0..CHUNK_EDGE {
        for y in 0..CHUNK_EDGE {
            for x in 0..CHUNK_EDGE {
                if occupied(x, y, z) {
                    set_mip_bit(&mut words, occupancy_mip_index(0, x, y, z));
                }
            }
        }
    }
    for lod in 1..=MAX_CHUNK_LOD {
        let edge = CHUNK_EDGE >> lod;
        for z in 0..edge {
            for y in 0..edge {
                for x in 0..edge {
                    let occupied = (0..2).any(|dz| {
                        (0..2).any(|dy| {
                            (0..2).any(|dx| {
                                mip_bit(
                                    &words,
                                    occupancy_mip_index(
                                        lod - 1,
                                        x * 2 + dx,
                                        y * 2 + dy,
                                        z * 2 + dz,
                                    ),
                                )
                            })
                        })
                    });
                    if occupied {
                        set_mip_bit(&mut words, occupancy_mip_index(lod, x, y, z));
                    }
                }
            }
        }
    }
    words
}

fn indirect_bytes() -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
    bytes
}

fn world_compute(
    view: ViewQuery<(&ExtractedView, &ExtractedCamera)>,
    extracted: Res<ExtractedChunks>,
    pipelines: Res<WorldPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    settings: Res<WorldRenderSettings>,
    mut residency: ResMut<WorldResidency>,
    mut context: RenderContext,
) {
    if residency.chunks.is_empty() {
        return;
    }
    let started = Instant::now();
    let (view, camera) = view.into_inner();
    let viewport_height = camera
        .physical_viewport_size
        .or(camera.physical_target_size)
        .map_or(1.0, |size| size.y.max(1) as f32);
    let camera_position = view.world_from_view.translation();
    let projection_scale = view.clip_from_view.y_axis.y;
    let Some(pipeline) = pipeline_cache.get_compute_pipeline(pipelines.compute) else {
        return;
    };

    // LOD is a view-direction-independent state. Recompute it only when the
    // camera crosses a Chunk boundary or projection/scale changes; rotation does
    // not alter LodContext.
    let next_lod_contexts = residency
        .local_transforms
        .iter()
        .filter_map(|(id, transform)| {
            lod::context(
                *transform,
                camera_position,
                viewport_height,
                projection_scale,
            )
            .map(|context| (*id, context))
        })
        .collect::<HashMap<_, _>>();
    let changed_contexts = next_lod_contexts
        .iter()
        .filter_map(|(id, context)| {
            (residency.lod_contexts.get(id) != Some(context)).then_some(*id)
        })
        .collect::<HashSet<_>>();
    let lod_updates = residency
        .chunks
        .iter()
        .filter(|(id, _)| changed_contexts.contains(&id.local_coordinate_id))
        .map(|(id, chunk)| {
            let lod_context = next_lod_contexts.get(&id.local_coordinate_id);
            let selected = lod_context.map_or(0, |context| {
                lod::selected_lod(
                    id.coordinate,
                    context,
                    chunk.geometry.as_ref().map(|geometry| geometry.key.lod),
                )
            });
            (*id, selected)
        })
        .collect::<Vec<_>>();
    residency.lod_contexts = next_lod_contexts;
    for (id, selected) in lod_updates {
        if residency.chunks[&id].desired_lod != selected {
            residency.chunks.get_mut(&id).unwrap().desired_lod = selected;
            mark_chunk_and_neighbors_dirty(&mut residency.geometry_dirty, id);
        }
    }

    let dirty_geometry = std::mem::take(&mut residency.geometry_dirty);
    for id in dirty_geometry {
        let Some(chunk) = residency.chunks.get(&id) else {
            let was_pending = residency.build_needed.remove(&id);
            bevy::log::trace!(
                "discarded build marker for absent Chunk: chunk={id:?}, was_pending={was_pending}"
            );
            continue;
        };
        let key = GeometryKey {
            source_revision: chunk.revision,
            lod: chunk.desired_lod,
            boundary_signature: chunk_boundary_signature(id, &residency.chunks),
        };
        let matches = chunk
            .geometry
            .as_ref()
            .is_some_and(|geometry| geometry.key == key);
        residency.chunks.get_mut(&id).unwrap().desired_key = Some(key);
        if matches {
            let _was_pending = residency.build_needed.remove(&id);
        } else {
            residency.build_needed.insert(id);
        }
    }

    if let Some(pending) = residency.pending_count.as_ref()
        && pending.status.load(Ordering::Acquire) == 0
    {
        pending.status.store(1, Ordering::Release);
        let completion = Arc::clone(&pending.status);
        pending
            .readback
            .slice(..)
            .map_async(MapMode::Read, move |result| {
                completion.store(if result.is_ok() { 2 } else { 3 }, Ordering::Release);
            });
    }
    let counted_geometry = match residency.pending_count.as_ref() {
        Some(pending) if pending.status.load(Ordering::Acquire) == 2 => {
            let pending = residency.pending_count.take().unwrap();
            let mapped = pending.readback.slice(..).get_mapped_range();
            let counts = pending
                .entries
                .iter()
                .enumerate()
                .map(|(index, (id, key))| {
                    let offset = index * pending.stride as usize;
                    let count = u32::from_le_bytes(mapped[offset..offset + 4].try_into().unwrap());
                    (*id, *key, count)
                })
                .collect::<Vec<_>>();
            drop(mapped);
            pending.readback.unmap();
            Some(counts)
        }
        Some(pending) if pending.status.load(Ordering::Acquire) == 3 => {
            let pending = residency.pending_count.take().unwrap();
            for (id, key) in pending.entries {
                if let Some(chunk) = residency.chunks.get_mut(&id)
                    && chunk.pending_key == Some(key)
                {
                    chunk.pending_key = None;
                }
            }
            bevy::log::error!("world geometry count readback failed");
            None
        }
        _ => None,
    };

    let mut allocated_bytes = geometry_allocated_bytes(&residency.chunks);
    let mut frame_build_bytes = 0_u64;
    let mut admitted = Vec::new();
    for (id, key, maximum_vertices) in counted_geometry.into_iter().flatten() {
        if let Some(chunk) = residency.chunks.get_mut(&id)
            && chunk.pending_key == Some(key)
        {
            chunk.pending_key = None;
        }
        let desired_chunk = residency.chunks.get(&id);
        if desired_chunk.and_then(|chunk| chunk.desired_key) != Some(key) {
            continue;
        }
        let new_bytes = geometry_bytes_for_vertices(maximum_vertices);
        if frame_build_bytes.saturating_add(new_bytes) > settings.geometry_build_budget_bytes {
            residency.stats.geometry_budget_rejections += 1;
            continue;
        }
        let replaced_bytes = residency.chunks[&id]
            .geometry
            .as_ref()
            .map_or(0, |geometry| geometry.allocated_bytes);
        let required_bytes = allocated_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(new_bytes);
        if required_bytes > settings.geometry_budget_bytes {
            residency.stats.geometry_budget_rejections += 1;
            continue;
        }

        let requested_vertex_bytes = u64::from(maximum_vertices.max(1)) * GENERATED_VERTEX_SIZE;
        let reusable_allocation = residency.chunks[&id]
            .geometry
            .as_ref()
            .map(|geometry| geometry.allocation)
            .filter(|allocation| allocation.size >= requested_vertex_bytes);
        let (allocation, reused_allocation) = if let Some(allocation) = reusable_allocation {
            (allocation, true)
        } else {
            let Some(allocation) = residency
                .geometry_arena
                .as_mut()
                .expect("geometry arena is initialized during prepare")
                .allocate(&render_device, requested_vertex_bytes)
            else {
                residency.stats.geometry_budget_rejections += 1;
                continue;
            };
            (allocation, false)
        };
        let geometry_buffer = residency
            .geometry_arena
            .as_ref()
            .expect("geometry arena is initialized during prepare")
            .buffer(allocation)
            .clone();
        let neighbor_metadata = neighbor_svo_bytes(id, &residency.chunks, key.lod);
        render_queue.write_buffer(&residency.chunks[&id].uniform, 96, &neighbor_metadata);
        let geometry = create_geometry(
            &render_device,
            &pipeline_cache,
            &pipelines,
            &geometry_buffer,
            allocation,
            id,
            &residency.chunks,
            key.lod,
            key.boundary_signature,
            maximum_vertices,
        );
        let previous = {
            let chunk = residency.chunks.get_mut(&id).unwrap();
            render_queue.write_buffer(&chunk.uniform, 72, &maximum_vertices.to_le_bytes());
            render_queue.write_buffer(&chunk.uniform, 92, &i32::from(key.lod).to_le_bytes());
            chunk.geometry.replace(geometry)
        };
        allocated_bytes = required_bytes;
        if let Some(previous) = previous
            && !reused_allocation
        {
            residency.retired_geometry.push(RetiredGeometry {
                retired_at_epoch: extracted.epoch,
                geometry: previous,
            });
        }
        frame_build_bytes = frame_build_bytes.saturating_add(new_bytes);
        residency.stats.geometry_builds += 1;
        let was_pending = residency.build_needed.remove(&id);
        debug_assert!(
            was_pending,
            "admitted geometry build must have been pending"
        );
        admitted.push(id);
    }

    if residency.pending_count.is_none() {
        let mut chunks_to_count = residency
            .build_needed
            .iter()
            .filter_map(|id| {
                let chunk = residency.chunks.get(id)?;
                let key = chunk.desired_key?;
                (chunk.pending_key != Some(key)).then_some((
                    *id,
                    key,
                    chunk_distance_squared(chunk.world_from_chunk, camera_position),
                ))
            })
            .collect::<Vec<_>>();
        chunks_to_count.sort_by(|left, right| {
            left.2
                .total_cmp(&right.2)
                .then_with(|| chunk_id_key(left.0).cmp(&chunk_id_key(right.0)))
        });
        chunks_to_count.truncate(settings.max_geometry_builds_per_frame as usize);
        if !chunks_to_count.is_empty() {
            schedule_geometry_count(
                chunks_to_count,
                &render_device,
                &render_queue,
                &pipeline_cache,
                &pipelines,
                &mut residency,
                &mut context,
            );
        }
    }

    if !admitted.is_empty() {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("world occupancy mip LOD and greedy meshing"),
                timestamp_writes: None,
            });
        pass.set_pipeline(pipeline);
        for id in &admitted {
            let geometry = residency.chunks[id].geometry.as_ref().unwrap();
            pass.set_bind_group(0, &geometry.compute_bind_group, &[]);
            pass.dispatch_workgroups(FACE_PLANE_COUNT, 1, 1);
        }
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
            "world render: resident={}, visible={}, drawable={}, builds={}, budget_rejections={}, active_evictions={}, active_mib={}, compute_cpu_us={} (120-frame window)",
            residency.chunks.len(),
            residency
                .chunks
                .values()
                .filter(|chunk| chunk_visible(view, chunk.world_from_chunk))
                .count(),
            residency
                .chunks
                .values()
                .filter(|chunk| chunk.geometry.is_some())
                .count(),
            residency.stats.geometry_builds,
            residency.stats.geometry_budget_rejections,
            residency.stats.active_evictions,
            geometry_active_bytes(&residency.chunks) / (1024 * 1024),
            residency.stats.compute_cpu_micros,
        );
        residency.stats = WorldRenderStats {
            last_report_epoch: extracted.epoch,
            ..Default::default()
        };
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_geometry_count(
    chunks_to_count: Vec<(ChunkId, GeometryKey, f32)>,
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
    residency: &mut WorldResidency,
    context: &mut RenderContext,
) {
    let stride = u64::from(render_device.limits().min_storage_buffer_offset_alignment).max(16);
    let buffer_size = stride * chunks_to_count.len() as u64;
    let count_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world geometry counts"),
        contents: &vec![0; buffer_size as usize],
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
    });
    let readback = render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
        label: Some("world geometry count readback"),
        size: buffer_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let dummy_vertices =
        render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
            label: Some("world geometry count dummy vertex"),
            size: GENERATED_VERTEX_SIZE,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
    let layout = pipeline_cache.get_bind_group_layout(&pipelines.compute_layout);
    let mut bind_groups = Vec::with_capacity(chunks_to_count.len());
    for (index, (id, key, _)) in chunks_to_count.iter().enumerate() {
        let neighbor_metadata = neighbor_svo_bytes(*id, &residency.chunks, key.lod);
        let chunk = &residency.chunks[id];
        render_queue.write_buffer(&chunk.uniform, 72, &0_u32.to_le_bytes());
        render_queue.write_buffer(&chunk.uniform, 92, &i32::from(key.lod).to_le_bytes());
        render_queue.write_buffer(&chunk.uniform, 96, &neighbor_metadata);
        let neighbors = chunk_neighbors(*id, &residency.chunks);
        bind_groups.push(render_device.create_bind_group(
            Some("world geometry count bind group"),
            &layout,
            &BindGroupEntries::sequential((
                chunk.occupancy_mip.as_entire_binding(),
                dummy_vertices.as_entire_binding(),
                buffer_range_binding(&count_buffer, index as u64 * stride, 16),
                chunk.uniform.as_entire_binding(),
                neighbors[0].occupancy_mip.as_entire_binding(),
                neighbors[1].occupancy_mip.as_entire_binding(),
                neighbors[2].occupancy_mip.as_entire_binding(),
                neighbors[3].occupancy_mip.as_entire_binding(),
                neighbors[4].occupancy_mip.as_entire_binding(),
                neighbors[5].occupancy_mip.as_entire_binding(),
            )),
        ));
    }

    let pipeline = pipeline_cache
        .get_compute_pipeline(pipelines.compute)
        .expect("geometry count is scheduled only after the compute pipeline is ready");
    {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("world exact geometry count"),
                timestamp_writes: None,
            });
        pass.set_pipeline(pipeline);
        for bind_group in &bind_groups {
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(FACE_PLANE_COUNT, 1, 1);
        }
    }
    context
        .command_encoder()
        .copy_buffer_to_buffer(&count_buffer, 0, &readback, 0, buffer_size);

    // Mapping begins on the following render frame, after this frame's copy
    // command has been submitted. wgpu rejects submitting a buffer while a map
    // request is already pending.
    let status = Arc::new(AtomicU8::new(0));
    let entries = chunks_to_count
        .into_iter()
        .map(|(id, key, _)| {
            residency.chunks.get_mut(&id).unwrap().pending_key = Some(key);
            (id, key)
        })
        .collect();
    residency.pending_count = Some(PendingGeometryCount {
        entries,
        readback,
        stride,
        status,
    });
}

fn geometry_allocated_bytes(chunks: &HashMap<ChunkId, ResidentChunk>) -> u64 {
    geometry_active_bytes(chunks)
}

fn geometry_active_bytes(chunks: &HashMap<ChunkId, ResidentChunk>) -> u64 {
    chunks
        .values()
        .filter_map(|chunk| chunk.geometry.as_ref())
        .map(|geometry| geometry.allocated_bytes)
        .sum()
}

fn create_geometry(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
    geometry_buffer: &Buffer,
    allocation: GeometryAllocation,
    id: ChunkId,
    chunks: &HashMap<ChunkId, ResidentChunk>,
    lod: u8,
    boundary_signature: u64,
    maximum_vertices: u32,
) -> ResidentGeometry {
    let chunk = &chunks[&id];
    let indirect = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world chunk indirect draw"),
        contents: &indirect_bytes(),
        usage: BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST,
    });
    let neighbors = chunk_neighbors(id, chunks);
    let compute_bind_group = render_device.create_bind_group(
        Some("world chunk compute bind group"),
        &pipeline_cache.get_bind_group_layout(&pipelines.compute_layout),
        &BindGroupEntries::sequential((
            chunk.occupancy_mip.as_entire_binding(),
            geometry_binding(geometry_buffer, allocation),
            indirect.as_entire_binding(),
            chunk.uniform.as_entire_binding(),
            neighbors[0].occupancy_mip.as_entire_binding(),
            neighbors[1].occupancy_mip.as_entire_binding(),
            neighbors[2].occupancy_mip.as_entire_binding(),
            neighbors[3].occupancy_mip.as_entire_binding(),
            neighbors[4].occupancy_mip.as_entire_binding(),
            neighbors[5].occupancy_mip.as_entire_binding(),
        )),
    );
    let render_bind_group = create_render_bind_group(
        render_device,
        pipeline_cache,
        pipelines,
        geometry_buffer,
        allocation,
        &chunk.uniform,
    );
    ResidentGeometry {
        key: GeometryKey {
            source_revision: chunk.revision,
            lod,
            boundary_signature,
        },
        allocated_bytes: geometry_bytes_for_vertices(maximum_vertices),
        allocation,
        indirect,
        compute_bind_group,
        render_bind_group,
    }
}

fn buffer_range_binding(buffer: &Buffer, offset: u64, size: u64) -> BindingResource<'_> {
    BindingResource::Buffer(BufferBinding {
        buffer,
        offset,
        size: BufferSize::new(size),
    })
}

fn geometry_binding(buffer: &Buffer, allocation: GeometryAllocation) -> BindingResource<'_> {
    buffer_range_binding(buffer, allocation.offset, allocation.size)
}

fn create_render_bind_group(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
    geometry_buffer: &Buffer,
    allocation: GeometryAllocation,
    uniform: &Buffer,
) -> BindGroup {
    render_device.create_bind_group(
        Some("world chunk render bind group"),
        &pipeline_cache.get_bind_group_layout(&pipelines.render_layout),
        &BindGroupEntries::sequential((
            geometry_binding(geometry_buffer, allocation),
            uniform.as_entire_binding(),
        )),
    )
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

fn chunk_neighbors<'a>(
    id: ChunkId,
    chunks: &'a HashMap<ChunkId, ResidentChunk>,
) -> [&'a ResidentChunk; 6] {
    let fallback = &chunks[&id];
    std::array::from_fn(|index| {
        neighbor_id(id, CHUNK_NEIGHBOR_OFFSETS[index])
            .and_then(|neighbor| {
                let chunk = chunks.get(&neighbor);
                chunk
            })
            .unwrap_or(fallback)
    })
}

fn neighbor_svo_bytes(
    id: ChunkId,
    chunks: &HashMap<ChunkId, ResidentChunk>,
    self_lod: u8,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(6 * 16);
    for offset in CHUNK_NEIGHBOR_OFFSETS {
        let neighbor = neighbor_id(id, offset).and_then(|neighbor| chunks.get(&neighbor));
        let neighbor_lod = neighbor.map_or(self_lod, |chunk| chunk.desired_lod);
        let metadata = neighbor.map_or([0, 0, 0, u32::from(self_lod)], |_| {
            [0, u32::from(MAX_CHUNK_LOD), 1, u32::from(neighbor_lod)]
        });
        for value in metadata {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn chunk_boundary_signature(id: ChunkId, chunks: &HashMap<ChunkId, ResidentChunk>) -> u64 {
    boundary_signature_from_neighbors(id, |neighbor| {
        let chunk = chunks.get(&neighbor);
        chunk.map(|chunk| (chunk.revision, chunk.desired_lod))
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

fn geometry_bytes_for_vertices(vertex_count: u32) -> u64 {
    u64::from(vertex_count.max(1)) * GENERATED_VERTEX_SIZE + 16
}

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

fn chunk_visible(view: &ExtractedView, world_from_chunk: [f32; 16]) -> bool {
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    aabb_intersects_clip(clip_from_world * Mat4::from_cols_array(&world_from_chunk))
}

fn chunk_id_key(id: ChunkId) -> (u64, i64, i64, i64) {
    (
        id.local_coordinate_id.0,
        id.coordinate[0],
        id.coordinate[1],
        id.coordinate[2],
    )
}

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
        &ExtractedView,
    )>,
    view_bind_group: Res<WorldViewBindGroup>,
    pipelines: Res<PipelineCache>,
    residency: Res<WorldResidency>,
    mut context: RenderContext,
) {
    if residency.chunks.is_empty() {
        return;
    }
    let (target, depth, view_offset, pipeline_id, extracted_view) = view.into_inner();
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
    for chunk in residency.chunks.values() {
        if !chunk_visible(extracted_view, chunk.world_from_chunk) {
            continue;
        }
        let Some(geometry) = &chunk.geometry else {
            continue;
        };
        pass.set_bind_group(1, &geometry.render_bind_group, &[]);
        pass.draw_indirect(&geometry.indirect, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_seed_has_the_documented_default() {
        let settings = WorldRenderSettings::default();
        assert_eq!(settings.material_seed, 0xDEAD_BEEF);
        assert_eq!(settings.geometry_budget_bytes, 1024 * 1024 * 1024);
        assert_eq!(settings.geometry_build_budget_bytes, 16 * 1024 * 1024);
        assert_eq!(settings.max_geometry_builds_per_frame, 32);
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
    fn occupancy_mip_ors_two_by_two_by_two_children() {
        let mip = build_occupancy_mip(|x, y, z| x == 3 && y == 7 && z == 5);
        for lod in 0..=MAX_CHUNK_LOD {
            let scale = 1_u32 << lod;
            assert!(mip_bit(
                &mip,
                occupancy_mip_index(lod, 3 / scale, 7 / scale, 5 / scale)
            ));
        }
        assert!(!mip_bit(&mip, occupancy_mip_index(1, 1, 7, 2)));
        assert_eq!(mip.len(), OCCUPANCY_MIP_WORD_COUNT);
        assert_eq!(
            OCCUPANCY_MIP_CELL_COUNT,
            16_u32.pow(3) + 8_u32.pow(3) + 4_u32.pow(3) + 2_u32.pow(3) + 1
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
            shader.contains("neighbor_chunks"),
            "meshing shader has no cross-chunk neighbor input"
        );
    }

    #[test]
    fn lod_meshing_uses_a_cubic_occupancy_pyramid() {
        let shader = include_str!("shaders/world_greedy_emit.wgsl");
        assert!(
            shader.contains("OCCUPANCY_MIP_OFFSETS"),
            "16³ chunks must query the complete cubic occupancy mip"
        );
        assert!(shader.contains("return vec3(16u >> lod)"));
        assert!(
            !shader.contains("occupied_voxel"),
            "coarse meshing must not rescan fine SVO voxels"
        );
        assert!(!shader.contains("horizontal"));
    }
}
