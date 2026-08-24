mod systems;

use systems::{CachedClientChunk, PendingChunkUpdate};

use crate::local_coordinate::{
    base::{LocalCoordinateBasePlugin, rebuild_dirty_chunk_triangles},
    data::LocalCoordinate,
    derived_svo::{DerivedSvoJob, DerivedSvoResult, DerivedSvoWorker},
    mesh::LocalCoordinateMeshPlugin,
    pcg::{remove_generated_chunk, replace_read_only_chunk},
    transform::LocalCoordinateTransform,
    virtual_chunk::rebuild_virtual_chunk_index,
};
use crate::{ChunkCoordinate, ChunkId, ChunkVersion, LocalCoordinateId, VoxelChunkSvo};
use avian3d::prelude::RigidBody;
use bevy::prelude::{
    App, Commands, Entity, IntoScheduleConfigs, Plugin, Query, Res, ResMut, Resource, Update,
};
use roundo_networking::SerializedPayload;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB, UpdateVersion,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const MAX_COMMANDS_INGESTED_PER_FRAME: usize = 32;
const MAX_CHUNK_UPDATES_APPLIED_PER_FRAME: usize = 16;

pub type LocalCoordinateClientIpc =
    CrossbeamThreadPipeEndpointA<LocalCoordinateClientCommand, LocalCoordinateClientEvent>;

#[derive(Clone)]
pub struct LocalCoordinateClientPlugin {
    pipe: CrossbeamThreadPipe<LocalCoordinateClientCommand, LocalCoordinateClientEvent>,
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

#[derive(Clone, Debug)]
pub enum LocalCoordinateClientCommand {
    BeginSession,
    ResetRequests,
    Spawn(LocalCoordinateId),
    Despawn(LocalCoordinateId),
    VersionUpdates(Vec<ChunkVersion>),
    LoadChunk {
        chunk: ChunkVersion,
        payload: SerializedPayload,
    },
    UnloadChunk {
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    },
}

#[derive(Clone, Debug)]
pub enum LocalCoordinateClientEvent {
    RequestChunks(Vec<ChunkId>),
}

#[derive(Resource)]
pub struct LocalCoordinateClientWorld {
    coordinates: HashMap<LocalCoordinateId, Entity>,
    cached_chunks: HashMap<ChunkId, CachedClientChunk>,
    active_server_versions: HashMap<ChunkId, UpdateVersion>,
    requested_server_versions: HashMap<ChunkId, UpdateVersion>,
    pending_chunk_updates: VecDeque<PendingChunkUpdate>,
    derived_svo: DerivedSvoWorker,
}

impl Default for LocalCoordinateClientWorld {
    fn default() -> Self {
        Self {
            coordinates: HashMap::new(),
            cached_chunks: HashMap::new(),
            active_server_versions: HashMap::new(),
            requested_server_versions: HashMap::new(),
            pending_chunk_updates: VecDeque::new(),
            derived_svo: DerivedSvoWorker::spawn("roundo-client-derived-svo"),
        }
    }
}

impl LocalCoordinateClientWorld {
    pub fn entity(&self, local_coordinate_id: LocalCoordinateId) -> Option<Entity> {
        self.coordinates.get(&local_coordinate_id).copied()
    }

    pub fn coordinate_count(&self) -> usize {
        self.coordinates.len()
    }

    pub fn cached_chunk_count(&self) -> usize {
        self.cached_chunks.len()
    }

    pub fn local_coordinate_id(&self, entity: Entity) -> Option<LocalCoordinateId> {
        self.coordinates
            .iter()
            .find_map(|(id, candidate)| (*candidate == entity).then_some(*id))
    }
}
