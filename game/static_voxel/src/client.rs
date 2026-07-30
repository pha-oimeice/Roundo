use crate::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, GeneratedChunk, StaticVoxelSvoView,
    derivation::{
        ChunkMeshRequest, ChunkMeshResult, ChunkSvoRequest, FACE_OFFSETS,
        StaticVoxelDerivationPipeline,
    },
};
use bevy::prelude::{
    App, Assets, Color, Commands, Component, Entity, Handle, IntoScheduleConfigs, Mesh, Mesh3d,
    MeshMaterial3d, Pickable, Plugin, Res, ResMut, Resource, StandardMaterial, Transform, Update,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_CHUNK_COMMANDS_INGESTED_PER_FRAME: usize = 32;
const MAX_DERIVATION_RESULTS_RECEIVED_PER_FRAME: usize = 64;
const MAX_READY_MESH_RESULTS_INSPECTED_PER_FRAME: usize = 64;
const MAX_MESHES_COMMITTED_PER_FRAME: usize = 4;
const MAX_MESH_VERTICES_COMMITTED_PER_FRAME: usize = 100_000;

pub type StaticVoxelClientIpc = CrossbeamThreadPipeEndpointA<StaticVoxelClientCommand, ()>;

#[derive(Clone)]
pub struct StaticVoxelClientPlugin {
    pipe: CrossbeamThreadPipe<StaticVoxelClientCommand, ()>,
}

impl StaticVoxelClientPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> StaticVoxelClientIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for StaticVoxelClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for StaticVoxelClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StaticVoxelClientWorld>()
            .init_resource::<StaticVoxelClientDerivation>()
            .insert_resource(StaticVoxelClientPipe(self.pipe.endpoint_b()))
            .add_systems(
                Update,
                (
                    ingest_static_voxel_commands,
                    collect_static_voxel_derivations,
                    dispatch_mesh_jobs,
                    dispatch_svo_job,
                )
                    .chain(),
            );
    }
}

#[derive(Clone, Debug)]
pub enum StaticVoxelClientCommand {
    LoadChunk(GeneratedChunk),
    UnloadChunk(ChunkCoordinate),
}

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticVoxelChunk {
    pub coordinate: ChunkCoordinate,
}

pub struct ClientStaticVoxelChunk {
    pub data: GeneratedChunk,
    pub svo_view: Option<StaticVoxelSvoView>,
    entity: Entity,
    mesh: Option<Handle<Mesh>>,
    mesh_revision: u64,
    mesh_job_pending: bool,
    mesh_job_in_flight: bool,
    svo_revision: u64,
    svo_job_pending: bool,
    svo_job_in_flight: bool,
}

impl ClientStaticVoxelChunk {
    pub fn entity(&self) -> Entity {
        self.entity
    }
}

#[derive(Resource, Default)]
pub struct StaticVoxelClientWorld {
    chunks: HashMap<ChunkCoordinate, ClientStaticVoxelChunk>,
    dirty_chunks: HashSet<ChunkCoordinate>,
    ready_meshes: VecDeque<ChunkMeshResult>,
    material: Option<Handle<StandardMaterial>>,
    next_revision: u64,
}

impl StaticVoxelClientWorld {
    pub fn chunk(&self, coordinate: ChunkCoordinate) -> Option<&ClientStaticVoxelChunk> {
        self.chunks.get(&coordinate)
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    fn allocate_revision(&mut self) -> u64 {
        self.next_revision = self.next_revision.wrapping_add(1);
        if self.next_revision == 0 {
            self.next_revision = 1;
        }
        self.next_revision
    }
}

#[derive(Resource, Clone)]
struct StaticVoxelClientPipe(CrossbeamThreadPipeEndpointB<StaticVoxelClientCommand, ()>);

#[derive(Resource)]
struct StaticVoxelClientDerivation(StaticVoxelDerivationPipeline);

impl Default for StaticVoxelClientDerivation {
    fn default() -> Self {
        Self(StaticVoxelDerivationPipeline::new())
    }
}

fn ingest_static_voxel_commands(
    mut commands: Commands,
    pipe: Res<StaticVoxelClientPipe>,
    mut world: ResMut<StaticVoxelClientWorld>,
) {
    for _ in 0..MAX_CHUNK_COMMANDS_INGESTED_PER_FRAME {
        let Some(command) = pipe.0.try_receive() else {
            break;
        };
        match command {
            StaticVoxelClientCommand::LoadChunk(chunk) => {
                let coordinate = chunk.coordinate;
                let svo_revision = world.allocate_revision();
                if let Some(existing) = world.chunks.get_mut(&coordinate) {
                    existing.data = chunk;
                    existing.svo_view = None;
                    existing.svo_revision = svo_revision;
                    existing.svo_job_pending = true;
                } else {
                    let edge = CHUNK_EDGE_LENGTH as f32;
                    let transform = Transform::from_xyz(
                        coordinate[0] as f32 * edge,
                        coordinate[1] as f32 * edge,
                        coordinate[2] as f32 * edge,
                    );
                    let entity = commands
                        .spawn((StaticVoxelChunk { coordinate }, transform, Pickable::IGNORE))
                        .id();
                    world.chunks.insert(
                        coordinate,
                        ClientStaticVoxelChunk {
                            data: chunk,
                            svo_view: None,
                            entity,
                            mesh: None,
                            mesh_revision: 0,
                            mesh_job_pending: false,
                            mesh_job_in_flight: false,
                            svo_revision,
                            svo_job_pending: true,
                            svo_job_in_flight: false,
                        },
                    );
                }
                mark_chunk_and_neighbors_dirty(&mut world.dirty_chunks, coordinate);
            }
            StaticVoxelClientCommand::UnloadChunk(coordinate) => {
                if let Some(chunk) = world.chunks.remove(&coordinate) {
                    commands.entity(chunk.entity).despawn();
                }
                mark_chunk_and_neighbors_dirty(&mut world.dirty_chunks, coordinate);
            }
        }
    }
    mark_dirty_mesh_jobs_pending(&mut world);
}

fn collect_static_voxel_derivations(
    mut commands: Commands,
    mut derivation: ResMut<StaticVoxelClientDerivation>,
    mut world: ResMut<StaticVoxelClientWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for _ in 0..MAX_DERIVATION_RESULTS_RECEIVED_PER_FRAME {
        let Some(result) = derivation.0.try_receive_mesh() else {
            break;
        };
        let is_current = if let Some(chunk) = world.chunks.get_mut(&result.coordinate) {
            chunk.mesh_job_in_flight = false;
            chunk.mesh_revision == result.revision
        } else {
            false
        };
        if is_current {
            world.ready_meshes.push_back(result);
        }
    }

    for _ in 0..MAX_DERIVATION_RESULTS_RECEIVED_PER_FRAME {
        let Some(result) = derivation.0.try_receive_svo() else {
            break;
        };
        if let Some(chunk) = world.chunks.get_mut(&result.coordinate) {
            chunk.svo_job_in_flight = false;
            if chunk.svo_revision == result.revision {
                chunk.svo_view = Some(result.svo);
            }
        }
    }

    commit_ready_meshes(&mut commands, &mut world, &mut meshes, &mut materials);
}

fn commit_ready_meshes(
    commands: &mut Commands,
    world: &mut StaticVoxelClientWorld,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let mut committed_meshes = 0;
    let mut committed_vertices: usize = 0;
    let mut inspected_results = 0;

    while inspected_results < MAX_READY_MESH_RESULTS_INSPECTED_PER_FRAME {
        let Some(result) = world.ready_meshes.pop_front() else {
            break;
        };
        inspected_results += 1;
        let Some(chunk) = world.chunks.get(&result.coordinate) else {
            continue;
        };
        if chunk.mesh_revision != result.revision {
            continue;
        }

        let vertex_count = result.mesh.as_ref().map_or(0, |mesh| mesh.vertex_count());
        if result.mesh.is_some() {
            if committed_meshes >= MAX_MESHES_COMMITTED_PER_FRAME
                || committed_meshes != 0
                    && committed_vertices.saturating_add(vertex_count)
                        > MAX_MESH_VERTICES_COMMITTED_PER_FRAME
            {
                world.ready_meshes.push_front(result);
                break;
            }
        }

        let entity = chunk.entity;
        let existing_handle = chunk.mesh.clone();
        match result.mesh {
            Some(mesh_data) => {
                let render_mesh = mesh_data.into_mesh();
                let mesh = if let Some(handle) = existing_handle
                    && let Some(mut existing_mesh) = meshes.get_mut(&handle)
                {
                    *existing_mesh = render_mesh;
                    handle
                } else {
                    meshes.add(render_mesh)
                };
                let material = world
                    .material
                    .get_or_insert_with(|| {
                        materials.add(StandardMaterial {
                            base_color: Color::srgb(0.45, 0.65, 0.4),
                            perceptual_roughness: 1.0,
                            unlit: true,
                            ..Default::default()
                        })
                    })
                    .clone();
                commands
                    .entity(entity)
                    .insert((Mesh3d(mesh.clone()), MeshMaterial3d(material)));
                if let Some(chunk) = world.chunks.get_mut(&result.coordinate) {
                    chunk.mesh = Some(mesh);
                }
            }
            None => {
                commands
                    .entity(entity)
                    .remove::<Mesh3d>()
                    .remove::<MeshMaterial3d<StandardMaterial>>();
                if let Some(chunk) = world.chunks.get_mut(&result.coordinate) {
                    chunk.mesh = None;
                }
            }
        }
        if vertex_count != 0 {
            committed_meshes += 1;
            committed_vertices = committed_vertices.saturating_add(vertex_count);
        }
    }
}

fn dispatch_mesh_jobs(
    mut derivation: ResMut<StaticVoxelClientDerivation>,
    mut world: ResMut<StaticVoxelClientWorld>,
) {
    let coordinates = world
        .chunks
        .iter()
        .filter_map(|(coordinate, chunk)| {
            (chunk.mesh_job_pending && !chunk.mesh_job_in_flight).then_some(*coordinate)
        })
        .take(derivation.0.available_mesh_job_slots())
        .collect::<Vec<_>>();

    for coordinate in coordinates {
        let Some(request) = mesh_request(&world, coordinate) else {
            continue;
        };
        if let Some(chunk) = world.chunks.get_mut(&coordinate) {
            chunk.mesh_job_pending = false;
            chunk.mesh_job_in_flight = true;
        }
        if !derivation.0.start_mesh_job(request)
            && let Some(chunk) = world.chunks.get_mut(&coordinate)
        {
            chunk.mesh_job_pending = true;
            chunk.mesh_job_in_flight = false;
        }
    }
}

fn dispatch_svo_job(
    pipe: Res<StaticVoxelClientPipe>,
    mut derivation: ResMut<StaticVoxelClientDerivation>,
    mut world: ResMut<StaticVoxelClientWorld>,
) {
    if !pipe.0.is_empty()
        || derivation.0.has_mesh_jobs_in_flight()
        || !world.ready_meshes.is_empty()
        || world
            .chunks
            .values()
            .any(|chunk| chunk.mesh_job_pending || chunk.mesh_job_in_flight)
        || !derivation.0.can_start_svo_job()
    {
        return;
    }

    let Some(coordinate) = world.chunks.iter().find_map(|(coordinate, chunk)| {
        (chunk.svo_job_pending && !chunk.svo_job_in_flight).then_some(*coordinate)
    }) else {
        return;
    };
    let Some(chunk) = world.chunks.get(&coordinate) else {
        return;
    };
    let request = ChunkSvoRequest {
        coordinate,
        revision: chunk.svo_revision,
        chunk: chunk.data.clone(),
    };
    if let Some(chunk) = world.chunks.get_mut(&coordinate) {
        chunk.svo_job_pending = false;
        chunk.svo_job_in_flight = true;
    }
    if !derivation.0.start_svo_job(request)
        && let Some(chunk) = world.chunks.get_mut(&coordinate)
    {
        chunk.svo_job_pending = true;
        chunk.svo_job_in_flight = false;
    }
}

fn mesh_request(
    world: &StaticVoxelClientWorld,
    coordinate: ChunkCoordinate,
) -> Option<ChunkMeshRequest> {
    let chunk = world.chunks.get(&coordinate)?;
    let neighbors = FACE_OFFSETS.map(|offset| {
        let neighbor_coordinate = std::array::from_fn(|axis| coordinate[axis] + offset[axis]);
        world
            .chunks
            .get(&neighbor_coordinate)
            .map(|neighbor| neighbor.data.clone())
    });
    Some(ChunkMeshRequest {
        coordinate,
        revision: chunk.mesh_revision,
        center: chunk.data.clone(),
        neighbors,
    })
}

fn mark_dirty_mesh_jobs_pending(world: &mut StaticVoxelClientWorld) {
    let dirty = std::mem::take(&mut world.dirty_chunks);
    for coordinate in dirty {
        let revision = world.allocate_revision();
        if let Some(chunk) = world.chunks.get_mut(&coordinate) {
            chunk.mesh_revision = revision;
            chunk.mesh_job_pending = true;
        }
    }
}

fn mark_chunk_and_neighbors_dirty(
    dirty: &mut HashSet<ChunkCoordinate>,
    coordinate: ChunkCoordinate,
) {
    dirty.insert(coordinate);
    for offset in FACE_OFFSETS {
        dirty.insert(std::array::from_fn(|axis| coordinate[axis] + offset[axis]));
    }
}
