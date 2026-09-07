use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    core_pipeline::{
        core_3d::main_opaque_pass_3d,
        schedule::{Core3d, Core3dSystems},
    },
    prelude::{
        App, Commands, Component, Entity, GlobalTransform, IVec4, IntoScheduleConfigs, Mat4, Query,
        Res, ResMut, Resource, UVec4, Vec4,
    },
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BlendState, Buffer, BufferInitDescriptor, BufferUsages, CachedComputePipelineId,
            CachedRenderPipelineId, ColorTargetState, ColorWrites, CompareFunction,
            ComputePassDescriptor, ComputePipelineDescriptor, DepthStencilState, FragmentState,
            FrontFace, MultisampleState, PipelineCache, PolygonMode, PrimitiveState,
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
use roundo_algorithm::tree::PackedSvo;
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, ChunkId, LocalCoordinate, LocalCoordinateIdentity, VoxelChunkSvo,
};
use std::{borrow::Cow, collections::HashMap, sync::Arc};

pub const DEFAULT_WORLD_MATERIAL_SEED: u32 = 0xDEAD_BEEF;
const DEFAULT_RETIREMENT_FRAMES: u64 = 3;
const DEFAULT_GEOMETRY_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const CHUNK_EDGE: u32 = CHUNK_EDGE_LENGTH as u32;
const FACE_PLANE_COUNT: u32 = 6 * CHUNK_EDGE;
const MAX_CHUNK_LOD: u8 = CHUNK_EDGE.ilog2() as u8;
const GENERATED_VERTEX_SIZE: u64 = 16;
const MAX_GEOMETRY_BUILDS_PER_FRAME: usize = 32;

#[derive(Clone, Copy, Debug, ExtractResource, Resource)]
pub struct WorldRenderSettings {
    pub material_seed: u32,
    pub retirement_frames: u64,
    pub geometry_budget_bytes: u64,
}

impl Default for WorldRenderSettings {
    fn default() -> Self {
        Self {
            material_seed: DEFAULT_WORLD_MATERIAL_SEED,
            retirement_frames: DEFAULT_RETIREMENT_FRAMES,
            geometry_budget_bytes: DEFAULT_GEOMETRY_BUDGET_BYTES,
        }
    }
}

/// GPU-driven renderer for every loaded local-coordinate Chunk.
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
    chunks: Vec<ExtractedChunk>,
}

struct ExtractedChunk {
    id: ChunkId,
    revision: u64,
    svo: Arc<VoxelChunkSvo>,
    world_from_chunk: [f32; 16],
}

fn extract_loaded_chunks(
    mut extracted: ResMut<ExtractedChunks>,
    chunks: Extract<Query<(&LocalCoordinateIdentity, &LocalCoordinate, &GlobalTransform)>>,
) {
    extracted.epoch = extracted.epoch.wrapping_add(1);
    extracted.chunks.clear();

    for (identity, local_coordinate, global_transform) in &chunks {
        for (position, chunk) in &local_coordinate.chunks {
            let Some(svo) = chunk.compressed_svo() else {
                continue;
            };
            let chunk_translation = bevy::math::Affine3A::from_translation(
                (*position * CHUNK_EDGE_LENGTH as i32).as_vec3(),
            );
            let world_from_chunk = global_transform.affine() * chunk_translation;
            extracted.chunks.push(ExtractedChunk {
                id: ChunkId {
                    local_coordinate_id: identity.0,
                    coordinate: [
                        i64::from(position.x),
                        i64::from(position.y),
                        i64::from(position.z),
                    ],
                },
                revision: chunk.content_revision,
                svo: Arc::clone(svo),
                world_from_chunk: bevy::math::Mat4::from(world_from_chunk).to_cols_array(),
            });
        }
    }
}

#[derive(Resource, Default)]
struct WorldResidency {
    chunks: HashMap<ChunkId, ResidentChunk>,
    retired: Vec<RetiredChunk>,
    retired_geometry: Vec<RetiredGeometry>,
    upload_count: u64,
}

struct ResidentChunk {
    revision: u64,
    svo: Buffer,
    svo_root: u32,
    svo_maximum_depth: u32,
    uniform: Buffer,
    geometry: Option<ResidentGeometry>,
    world_from_chunk: [f32; 16],
    last_seen_epoch: u64,
}

struct ResidentGeometry {
    lod: u8,
    boundary_signature: u64,
    allocated_bytes: u64,
    indirect: Buffer,
    compute_bind_group: BindGroup,
    render_bind_group: BindGroup,
}

struct RetiredChunk {
    retired_at_epoch: u64,
    _chunk: ResidentChunk,
}

struct RetiredGeometry {
    retired_at_epoch: u64,
    _geometry: ResidentGeometry,
}

#[derive(Clone, Copy, ShaderType)]
struct GpuPackedSvoNode {
    words: UVec4,
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
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer::<GpuGeneratedVertex>(false),
                storage_buffer::<GpuDrawIndirect>(false),
                uniform_buffer::<WorldChunkUniform>(false),
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
                storage_buffer_read_only::<GpuPackedSvoNode>(false),
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

fn prepare_residency(
    extracted: Res<ExtractedChunks>,
    settings: Res<WorldRenderSettings>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut residency: ResMut<WorldResidency>,
) {
    let epoch = extracted.epoch;
    for observed in &extracted.chunks {
        if let Some(resident) = residency.chunks.get_mut(&observed.id) {
            resident.last_seen_epoch = epoch;
            if resident.revision == observed.revision {
                if resident.world_from_chunk != observed.world_from_chunk {
                    resident.world_from_chunk = observed.world_from_chunk;
                    let matrix = world_matrix_bytes(observed.world_from_chunk);
                    render_queue.write_buffer(&resident.uniform, 0, &matrix);
                }
                continue;
            }
        }

        let packed = observed.svo.pack_with(|voxel| voxel.0);
        let replacement = resident_chunk(
            &render_device,
            observed,
            &packed,
            settings.material_seed,
            epoch,
        );
        residency.upload_count = residency.upload_count.wrapping_add(1);
        if let Some(previous) = residency.chunks.insert(observed.id, replacement) {
            residency.retired.push(RetiredChunk {
                retired_at_epoch: epoch,
                _chunk: previous,
            });
        }
    }

    let unloaded = residency
        .chunks
        .iter()
        .filter_map(|(id, resident)| (resident.last_seen_epoch != epoch).then_some(*id))
        .collect::<Vec<_>>();
    for id in unloaded {
        if let Some(previous) = residency.chunks.remove(&id) {
            residency.retired.push(RetiredChunk {
                retired_at_epoch: epoch,
                _chunk: previous,
            });
        }
    }

    let retirement_frames = settings.retirement_frames.max(1);
    residency
        .retired
        .retain(|retired| epoch.wrapping_sub(retired.retired_at_epoch) < retirement_frames);
    residency
        .retired_geometry
        .retain(|retired| epoch.wrapping_sub(retired.retired_at_epoch) < retirement_frames);
}

fn resident_chunk(
    render_device: &RenderDevice,
    observed: &ExtractedChunk,
    packed: &PackedSvo,
    material_seed: u32,
    epoch: u64,
) -> ResidentChunk {
    let svo_bytes = packed.node_bytes();
    let svo = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world chunk SVO"),
        contents: &svo_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
    });
    let uniform_bytes = chunk_uniform_bytes(observed, material_seed, packed);
    let uniform = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("world chunk uniform"),
        contents: &uniform_bytes,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
    });
    ResidentChunk {
        revision: observed.revision,
        svo,
        svo_root: packed.root_index(),
        svo_maximum_depth: u32::from(packed.maximum_depth()),
        uniform,
        geometry: None,
        world_from_chunk: observed.world_from_chunk,
        last_seen_epoch: epoch,
    }
}

fn chunk_uniform_bytes(
    observed: &ExtractedChunk,
    material_seed: u32,
    packed: &PackedSvo,
) -> Vec<u8> {
    let mut bytes = world_matrix_bytes(observed.world_from_chunk);
    bytes.reserve(32);
    let metadata = [
        packed.root_index(),
        u32::from(packed.maximum_depth()),
        0,
        material_seed,
    ];
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

fn world_matrix_bytes(matrix: [f32; 16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    for value in matrix {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
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
    let Some(pipeline) = pipeline_cache.get_compute_pipeline(pipelines.compute) else {
        return;
    };
    if residency.chunks.is_empty() {
        return;
    }
    let (view, camera) = view.into_inner();
    let viewport_height = camera
        .physical_viewport_size
        .or(camera.physical_target_size)
        .map_or(1.0, |size| size.y.max(1) as f32);
    let camera_position = view.world_from_view.translation();
    let projection_scale = view.clip_from_view.y_axis.y;

    let chunks_to_mesh = residency
        .chunks
        .iter()
        .filter_map(|(id, chunk)| {
            if !chunk_visible(view, chunk.world_from_chunk) {
                return None;
            }
            let lod = selected_lod(
                chunk.world_from_chunk,
                camera_position,
                viewport_height,
                projection_scale,
                chunk.geometry.as_ref().map(|geometry| geometry.lod),
            );
            let boundary_signature = chunk_boundary_signature(*id, &residency.chunks);
            (chunk.geometry.as_ref().is_none_or(|geometry| {
                geometry.lod != lod || geometry.boundary_signature != boundary_signature
            }))
            .then_some((
                *id,
                lod,
                Mat4::from_cols_array(&chunk.world_from_chunk)
                    .transform_point3(bevy::prelude::Vec3::splat(8.0))
                    .distance_squared(camera_position),
                boundary_signature,
            ))
        })
        .collect::<Vec<_>>();
    if chunks_to_mesh.is_empty() {
        return;
    }
    let mut chunks_to_mesh = chunks_to_mesh;
    chunks_to_mesh.sort_by(|left, right| left.2.total_cmp(&right.2));
    let mut allocated_bytes = residency
        .chunks
        .values()
        .filter_map(|chunk| chunk.geometry.as_ref())
        .map(|geometry| geometry.allocated_bytes)
        .sum::<u64>()
        + residency
            .retired_geometry
            .iter()
            .map(|retired| retired._geometry.allocated_bytes)
            .sum::<u64>();
    let mut admitted = Vec::new();

    for (id, lod, _, boundary_signature) in
        chunks_to_mesh.iter().take(MAX_GEOMETRY_BUILDS_PER_FRAME)
    {
        let new_bytes = geometry_bytes_for_lod(*lod);
        if allocated_bytes.saturating_add(new_bytes) > settings.geometry_budget_bytes {
            continue;
        }
        allocated_bytes += new_bytes;
        let neighbor_metadata = neighbor_svo_bytes(*id, &residency.chunks);
        render_queue.write_buffer(&residency.chunks[id].uniform, 96, &neighbor_metadata);
        let geometry = create_geometry(
            &render_device,
            &pipeline_cache,
            &pipelines,
            *id,
            &residency.chunks,
            *lod,
            *boundary_signature,
        );
        let previous = {
            let chunk = residency.chunks.get_mut(id).unwrap();
            let maximum_vertices = maximum_vertices_for_lod(*lod);
            render_queue.write_buffer(&chunk.uniform, 72, &maximum_vertices.to_le_bytes());
            render_queue.write_buffer(&chunk.uniform, 92, &i32::from(*lod).to_le_bytes());
            chunk.geometry.replace(geometry)
        };
        if let Some(previous) = previous {
            residency.retired_geometry.push(RetiredGeometry {
                retired_at_epoch: extracted.epoch,
                _geometry: previous,
            });
        }
        admitted.push(*id);
    }
    if admitted.is_empty() {
        return;
    }

    let mut pass = context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("world SVO LOD face mask and greedy meshing"),
            timestamp_writes: None,
        });
    pass.set_pipeline(pipeline);
    for id in admitted {
        let geometry = residency.chunks[&id].geometry.as_ref().unwrap();
        pass.set_bind_group(0, &geometry.compute_bind_group, &[]);
        pass.dispatch_workgroups(FACE_PLANE_COUNT, 1, 1);
    }
}

fn create_geometry(
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &WorldPipelines,
    id: ChunkId,
    chunks: &HashMap<ChunkId, ResidentChunk>,
    lod: u8,
    boundary_signature: u64,
) -> ResidentGeometry {
    let chunk = &chunks[&id];
    let vertices = render_device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
        label: Some("world chunk generated vertices"),
        size: u64::from(maximum_vertices_for_lod(lod)) * GENERATED_VERTEX_SIZE,
        usage: BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
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
            chunk.svo.as_entire_binding(),
            vertices.as_entire_binding(),
            indirect.as_entire_binding(),
            chunk.uniform.as_entire_binding(),
            neighbors[0].svo.as_entire_binding(),
            neighbors[1].svo.as_entire_binding(),
            neighbors[2].svo.as_entire_binding(),
            neighbors[3].svo.as_entire_binding(),
            neighbors[4].svo.as_entire_binding(),
            neighbors[5].svo.as_entire_binding(),
        )),
    );
    let render_bind_group = render_device.create_bind_group(
        Some("world chunk render bind group"),
        &pipeline_cache.get_bind_group_layout(&pipelines.render_layout),
        &BindGroupEntries::sequential((
            vertices.as_entire_binding(),
            chunk.uniform.as_entire_binding(),
        )),
    );
    ResidentGeometry {
        lod,
        boundary_signature,
        allocated_bytes: geometry_bytes_for_lod(lod),
        indirect,
        compute_bind_group,
        render_bind_group,
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

fn chunk_neighbors<'a>(
    id: ChunkId,
    chunks: &'a HashMap<ChunkId, ResidentChunk>,
) -> [&'a ResidentChunk; 6] {
    let fallback = &chunks[&id];
    std::array::from_fn(|index| {
        neighbor_id(id, CHUNK_NEIGHBOR_OFFSETS[index])
            .and_then(|neighbor| chunks.get(&neighbor))
            .unwrap_or(fallback)
    })
}

fn neighbor_svo_bytes(id: ChunkId, chunks: &HashMap<ChunkId, ResidentChunk>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(6 * 16);
    for offset in CHUNK_NEIGHBOR_OFFSETS {
        let neighbor = neighbor_id(id, offset).and_then(|neighbor| chunks.get(&neighbor));
        let metadata = neighbor.map_or([0, 0, 0, 0], |neighbor| {
            [neighbor.svo_root, neighbor.svo_maximum_depth, 1, 0]
        });
        for value in metadata {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn chunk_boundary_signature(id: ChunkId, chunks: &HashMap<ChunkId, ResidentChunk>) -> u64 {
    CHUNK_NEIGHBOR_OFFSETS
        .into_iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |signature, offset| {
            let revision = neighbor_id(id, offset)
                .and_then(|neighbor| chunks.get(&neighbor))
                .map_or(0, |neighbor| neighbor.revision.wrapping_add(1));
            (signature ^ revision).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn maximum_vertices_for_lod(lod: u8) -> u32 {
    let edge = CHUNK_EDGE >> lod;
    6 * 6 * edge * edge * edge
}

fn geometry_bytes_for_lod(lod: u8) -> u64 {
    u64::from(maximum_vertices_for_lod(lod)) * GENERATED_VERTEX_SIZE + 16
}

fn chunk_visible(view: &ExtractedView, world_from_chunk: [f32; 16]) -> bool {
    let world_from_view = view.world_from_view.to_matrix();
    let clip_from_world = view
        .clip_from_world
        .unwrap_or(view.clip_from_view * world_from_view.inverse());
    let clip_from_chunk = clip_from_world * Mat4::from_cols_array(&world_from_chunk);
    aabb_intersects_clip(clip_from_chunk)
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

fn selected_lod(
    world_from_chunk: [f32; 16],
    camera_position: bevy::prelude::Vec3,
    viewport_height: f32,
    projection_scale: f32,
    current_lod: Option<u8>,
) -> u8 {
    let transform = Mat4::from_cols_array(&world_from_chunk);
    let center = transform.transform_point3(bevy::prelude::Vec3::splat(8.0));
    let distance = center.distance(camera_position).max(0.001);
    let world_cell_size = transform.x_axis.truncate().length();
    let base_pixels = world_cell_size * viewport_height * projection_scale / (2.0 * distance);
    let Some(mut lod) = current_lod else {
        let mut lod = 0_u8;
        while lod < MAX_CHUNK_LOD && base_pixels * ((1_u32 << lod) as f32) < 1.5 {
            lod += 1;
        }
        return lod;
    };
    while lod < MAX_CHUNK_LOD && base_pixels * ((1_u32 << lod) as f32) < 1.25 {
        lod += 1;
    }
    while lod > 0 && base_pixels * ((1_u32 << (lod - 1)) as f32) > 1.75 {
        lod -= 1;
    }
    lod
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
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: PolygonMode::Fill,
                    conservative: false,
                },
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
    }

    #[test]
    fn indirect_draw_starts_with_one_instance_and_no_vertices() {
        let bytes = indirect_bytes();
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
    }

    #[test]
    fn geometry_capacity_tracks_only_the_selected_lod() {
        assert_eq!(maximum_vertices_for_lod(0), 147_456);
        assert_eq!(maximum_vertices_for_lod(1), 18_432);
        assert_eq!(maximum_vertices_for_lod(2), 2_304);
        assert_eq!(maximum_vertices_for_lod(3), 288);
        assert_eq!(maximum_vertices_for_lod(4), 36);
    }

    #[test]
    fn clip_culling_rejects_chunks_fully_outside_one_plane() {
        assert!(aabb_intersects_clip(Mat4::IDENTITY));
        assert!(!aabb_intersects_clip(Mat4::from_translation(
            bevy::prelude::Vec3::new(100.0, 0.0, 0.0)
        )));
    }

    #[test]
    fn projected_lod_changes_with_distance_and_returns_when_approaching() {
        let transform = Mat4::IDENTITY.to_cols_array();
        let near = selected_lod(
            transform,
            bevy::prelude::Vec3::new(8.0, 24.0, 8.0),
            1080.0,
            1.7,
            None,
        );
        let far = selected_lod(
            transform,
            bevy::prelude::Vec3::new(8.0, 1_000.0, 8.0),
            1080.0,
            1.7,
            Some(near),
        );
        let returned = selected_lod(
            transform,
            bevy::prelude::Vec3::new(8.0, 24.0, 8.0),
            1080.0,
            1.7,
            Some(far),
        );
        assert_eq!(near, 0);
        assert!(far > near);
        assert_eq!(returned, near);
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
    fn lod_meshing_preserves_non_uniform_surface_cells() {
        let shader = include_str!("shaders/world_greedy_emit.wgsl");
        assert!(
            !shader.contains("node.child_mask == 0u && node.data != 0u"),
            "coarse occupancy discards every non-uniform surface cell"
        );
    }
}
