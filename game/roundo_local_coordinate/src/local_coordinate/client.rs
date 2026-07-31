use crate::local_coordinate::{
    base::{LocalCoordinateBasePlugin, rebuild_dirty_chunk_triangles},
    data::LocalCoordinate,
    pcg::{remove_generated_chunk, replace_generated_chunk},
    render::LocalCoordinateRenderPlugin,
    virtual_chunk::rebuild_virtual_chunk_index,
};
use crate::{ChunkCoordinate, GeneratedChunk, LocalCoordinateId};
use avian3d::prelude::RigidBody;
use bevy::prelude::{
    App, Commands, Entity, IntoScheduleConfigs, Plugin, Query, Res, ResMut, Resource, Transform,
    Update,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, VecDeque};

const MAX_COMMANDS_INGESTED_PER_FRAME: usize = 32;
const MAX_CHUNK_UPDATES_APPLIED_PER_FRAME: usize = 16;

pub type LocalCoordinateClientIpc = CrossbeamThreadPipeEndpointA<LocalCoordinateClientCommand, ()>;

#[derive(Clone)]
pub struct LocalCoordinateClientPlugin {
    pipe: CrossbeamThreadPipe<LocalCoordinateClientCommand, ()>,
}

impl LocalCoordinateClientPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> LocalCoordinateClientIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for LocalCoordinateClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for LocalCoordinateClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LocalCoordinateBasePlugin, LocalCoordinateRenderPlugin))
            .init_resource::<LocalCoordinateClientWorld>()
            .insert_resource(LocalCoordinateClientPipe(self.pipe.endpoint_b()))
            .add_systems(
                Update,
                (
                    ingest_commands,
                    apply_pending_chunk_updates,
                    rebuild_virtual_chunk_index,
                )
                    .chain()
                    .before(rebuild_dirty_chunk_triangles),
            );
    }
}

#[derive(Clone, Debug)]
pub enum LocalCoordinateClientCommand {
    Spawn(LocalCoordinateId),
    Despawn(LocalCoordinateId),
    LoadChunk {
        local_coordinate_id: LocalCoordinateId,
        chunk: GeneratedChunk,
    },
    UnloadChunk {
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    },
}

#[derive(Resource, Default)]
pub struct LocalCoordinateClientWorld {
    coordinates: HashMap<LocalCoordinateId, Entity>,
    pending_chunk_updates: VecDeque<PendingChunkUpdate>,
}

impl LocalCoordinateClientWorld {
    pub fn entity(&self, local_coordinate_id: LocalCoordinateId) -> Option<Entity> {
        self.coordinates.get(&local_coordinate_id).copied()
    }

    pub fn coordinate_count(&self) -> usize {
        self.coordinates.len()
    }

    pub fn local_coordinate_id(&self, entity: Entity) -> Option<LocalCoordinateId> {
        self.coordinates
            .iter()
            .find_map(|(id, candidate)| (*candidate == entity).then_some(*id))
    }
}

#[derive(Resource, Clone)]
struct LocalCoordinateClientPipe(CrossbeamThreadPipeEndpointB<LocalCoordinateClientCommand, ()>);

enum PendingChunkUpdate {
    Load {
        local_coordinate_id: LocalCoordinateId,
        chunk: GeneratedChunk,
    },
    Unload {
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    },
}

fn ingest_commands(
    mut commands: Commands,
    pipe: Res<LocalCoordinateClientPipe>,
    mut world: ResMut<LocalCoordinateClientWorld>,
) {
    for _ in 0..MAX_COMMANDS_INGESTED_PER_FRAME {
        let Some(command) = pipe.0.try_receive() else {
            break;
        };

        match command {
            LocalCoordinateClientCommand::Spawn(local_coordinate_id) => {
                ensure_coordinate(&mut commands, &mut world, local_coordinate_id);
            }
            LocalCoordinateClientCommand::Despawn(local_coordinate_id) => {
                if let Some(entity) = world.coordinates.remove(&local_coordinate_id) {
                    commands.entity(entity).despawn();
                }
                world
                    .pending_chunk_updates
                    .retain(|update| pending_local_coordinate_id(update) != local_coordinate_id);
            }
            LocalCoordinateClientCommand::LoadChunk {
                local_coordinate_id,
                chunk,
            } => {
                ensure_coordinate(&mut commands, &mut world, local_coordinate_id);
                world
                    .pending_chunk_updates
                    .push_back(PendingChunkUpdate::Load {
                        local_coordinate_id,
                        chunk,
                    });
            }
            LocalCoordinateClientCommand::UnloadChunk {
                local_coordinate_id,
                coordinate,
            } => {
                world
                    .pending_chunk_updates
                    .push_back(PendingChunkUpdate::Unload {
                        local_coordinate_id,
                        coordinate,
                    });
            }
        }
    }
}

fn ensure_coordinate(
    commands: &mut Commands,
    world: &mut LocalCoordinateClientWorld,
    local_coordinate_id: LocalCoordinateId,
) -> Entity {
    *world
        .coordinates
        .entry(local_coordinate_id)
        .or_insert_with(|| {
            commands
                .spawn((
                    LocalCoordinate::default(),
                    RigidBody::Static,
                    Transform::default(),
                ))
                .id()
        })
}

fn apply_pending_chunk_updates(
    mut world: ResMut<LocalCoordinateClientWorld>,
    mut local_coordinates: Query<&mut LocalCoordinate>,
) {
    let update_count = world
        .pending_chunk_updates
        .len()
        .min(MAX_CHUNK_UPDATES_APPLIED_PER_FRAME);
    for _ in 0..update_count {
        let Some(update) = world.pending_chunk_updates.pop_front() else {
            break;
        };
        let local_coordinate_id = pending_local_coordinate_id(&update);
        let Some(entity) = world.coordinates.get(&local_coordinate_id).copied() else {
            continue;
        };
        let Ok(mut local_coordinate) = local_coordinates.get_mut(entity) else {
            world.pending_chunk_updates.push_back(update);
            continue;
        };

        match update {
            PendingChunkUpdate::Load { chunk, .. } => {
                replace_generated_chunk(&mut local_coordinate, &chunk);
            }
            PendingChunkUpdate::Unload { coordinate, .. } => {
                remove_generated_chunk(&mut local_coordinate, coordinate);
            }
        }
    }
}

fn pending_local_coordinate_id(update: &PendingChunkUpdate) -> LocalCoordinateId {
    match update {
        PendingChunkUpdate::Load {
            local_coordinate_id,
            ..
        }
        | PendingChunkUpdate::Unload {
            local_coordinate_id,
            ..
        } => *local_coordinate_id,
    }
}
