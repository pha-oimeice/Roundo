mod generation;
mod systems;

use generation::{ChunkGenerationJob, ChunkGenerationWorker};
use systems::{GenerationPlan, ObservationRegion, PlayerSubscription};

use crate::local_coordinate::{
    base::{LocalCoordinateBasePlugin, LocalCoordinateSet},
    data::LocalCoordinate,
    derived_svo::{DerivedSvoJob, DerivedSvoResult, DerivedSvoWorker},
    pcg::{remove_generated_chunk, replace_generated_chunk},
    physics::{
        LocalCoordinatePhysicsInterests, LocalCoordinatePhysicsPlugin, LocalCoordinatePhysicsSet,
    },
    transform::LocalCoordinateTransform,
    virtual_chunk::VirtualChunkIndex,
};
use crate::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkId, ChunkVersion, DEFAULT_CHUNK_VIEW_DISTANCE,
    GeneratedChunk, LocalCoordinateId, LocalCoordinateIdentity, MAX_CHUNK_VIEW_DISTANCE,
    MIN_CHUNK_VIEW_DISTANCE, SuperflatGenerator,
};
use avian3d::prelude::RigidBody;
#[cfg(test)]
use bevy::prelude::Transform;
use bevy::prelude::{
    App, Commands, Component, GlobalTransform, IntoScheduleConfigs, Plugin, Query, Res, ResMut,
    Resource, Startup, SystemSet, Update, With,
};
use roundo_networking::ConnectionId;
use roundo_networking::SerializedPayload;
use roundo_presence::{
    Player, PlayerId, PlayerScene, SceneId, ServerPlayer, ServerSceneWorlds, TorusSpace,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB, UpdateVersion,
};
use std::collections::{HashMap, HashSet, VecDeque};

pub const DEFAULT_PCG_LOCAL_COORDINATE_ID: LocalCoordinateId = LocalCoordinateId(1);
const MAX_CHUNKS_GENERATED_PER_TICK: usize = 16;
const MAX_CHUNK_GENERATION_JOBS_IN_FLIGHT: usize = 32;
const MAX_CHUNKS_EVICTED_PER_TICK: usize = 32;
const PHYSICS_CHUNK_RADIUS: f64 = 4.0;
const MAX_CHUNK_RESPONSES_PER_TICK_PER_CONNECTION: usize = 16;
const MAX_DERIVED_SVO_RESULTS_PER_TICK: usize = 16;
const MAX_DERIVED_SVO_JOBS_IN_FLIGHT: usize = 32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
enum LocalCoordinateServerStreamingSet {
    Prepare,
    Commit,
}

pub type LocalCoordinateServerIpc =
    CrossbeamThreadPipeEndpointA<LocalCoordinateServerCommand, LocalCoordinateServerEvent>;

#[derive(Clone)]
pub struct LocalCoordinateServerPlugin {
    pipe: CrossbeamThreadPipe<LocalCoordinateServerCommand, LocalCoordinateServerEvent>,
    generated_coordinates: Vec<PcgLocalCoordinate>,
}

impl LocalCoordinateServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                SuperflatGenerator::new(0),
            )],
        }
    }

    pub fn with_generator(generator: SuperflatGenerator) -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                generator,
            )],
        }
    }

    pub fn with_generated_coordinate(
        mut self,
        local_coordinate_id: LocalCoordinateId,
        generator: SuperflatGenerator,
    ) -> Self {
        if let Some(existing) = self
            .generated_coordinates
            .iter_mut()
            .find(|coordinate| coordinate.id == local_coordinate_id)
        {
            existing.generator = generator;
        } else {
            self.generated_coordinates
                .push(PcgLocalCoordinate::new(local_coordinate_id, generator));
        }
        self
    }

    pub fn ipc(&self) -> LocalCoordinateServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for LocalCoordinateServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct PcgLocalCoordinate {
    pub id: LocalCoordinateId,
    pub generator: SuperflatGenerator,
    pub scene_id: SceneId,
}

impl PcgLocalCoordinate {
    pub const fn new(id: LocalCoordinateId, generator: SuperflatGenerator) -> Self {
        Self {
            id,
            generator,
            scene_id: SceneId::S1,
        }
    }

    pub const fn in_scene(mut self, scene_id: SceneId) -> Self {
        self.scene_id = scene_id;
        self
    }
}

#[derive(Clone, Debug)]
pub enum LocalCoordinateServerCommand {
    SubscribePlayer {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    UnsubscribePlayer {
        connection_id: ConnectionId,
    },
    RequestChunks {
        connection_id: ConnectionId,
        chunks: Vec<ChunkId>,
    },
    SetChunkViewDistance {
        connection_id: ConnectionId,
        chunks: u16,
    },
}

#[derive(Clone, Debug)]
pub enum LocalCoordinateServerEvent {
    Spawned {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
    },
    Despawned {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
    },
    ChunkVersions {
        connection_id: ConnectionId,
        chunks: Vec<ChunkVersion>,
    },
    ChunkLoaded {
        connection_id: ConnectionId,
        chunk: ChunkVersion,
        payload: SerializedPayload,
    },
    ChunkUnloaded {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    },
}

#[derive(Resource)]
pub struct LocalCoordinateServerWorld {
    loaded_chunks: HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
    generation_plans: HashMap<LocalCoordinateId, GenerationPlan>,
    generation_jobs: HashSet<ChunkId>,
    generation_worker: ChunkGenerationWorker,
    chunk_versions: HashMap<ChunkId, UpdateVersion>,
    observation_by_player: HashMap<PlayerId, ObservationRegion>,
    subscriptions: HashMap<ConnectionId, PlayerSubscription>,
    requested_view_distances: HashMap<ConnectionId, u16>,
    derived_svo_jobs: HashMap<(ConnectionId, ChunkId), UpdateVersion>,
    derived_svo: DerivedSvoWorker,
}

impl Default for LocalCoordinateServerWorld {
    fn default() -> Self {
        Self {
            loaded_chunks: HashMap::new(),
            generation_plans: HashMap::new(),
            generation_jobs: HashSet::new(),
            generation_worker: ChunkGenerationWorker::spawn(),
            chunk_versions: HashMap::new(),
            observation_by_player: HashMap::new(),
            subscriptions: HashMap::new(),
            requested_view_distances: HashMap::new(),
            derived_svo_jobs: HashMap::new(),
            derived_svo: DerivedSvoWorker::spawn("roundo-server-derived-svo"),
        }
    }
}

impl LocalCoordinateServerWorld {
    pub fn chunk(
        &self,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    ) -> Option<&GeneratedChunk> {
        self.loaded_chunks
            .get(&local_coordinate_id)?
            .get(&coordinate)
    }

    pub fn loaded_chunk_count(&self, local_coordinate_id: LocalCoordinateId) -> usize {
        self.loaded_chunks
            .get(&local_coordinate_id)
            .map_or(0, HashMap::len)
    }

    pub fn coordinate_count(&self) -> usize {
        self.loaded_chunks.len()
    }
}
