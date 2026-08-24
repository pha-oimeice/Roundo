mod systems;

use systems::{ObservationRegion, PlayerSubscription};

use crate::local_coordinate::{
    base::{LocalCoordinateBasePlugin, LocalCoordinateSet},
    data::LocalCoordinate,
    derived_svo::{DerivedSvoJob, DerivedSvoResult, DerivedSvoWorker},
    pcg::replace_generated_chunk,
    physics::LocalCoordinatePhysicsPlugin,
    transform::LocalCoordinateTransform,
    virtual_chunk::{VirtualChunkIndex, rebuild_virtual_chunk_index},
};
use crate::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkId, ChunkVersion, GeneratedChunk, LocalCoordinateId,
    SuperflatGenerator,
};
use avian3d::prelude::RigidBody;
#[cfg(test)]
use bevy::prelude::Transform;
use bevy::prelude::{
    App, Commands, Component, FixedUpdate, GlobalTransform, IntoScheduleConfigs, Plugin, Query,
    Res, ResMut, Resource, Startup, With,
};
use roundo_networking::ConnectionId;
use roundo_networking::SerializedPayload;
use roundo_presence::{
    Player, PlayerId, PlayerScene, PresenceServerSet, SceneId, ServerPlayer, ServerSceneWorlds,
    TorusSpace,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB, UpdateVersion,
};
use std::collections::{HashMap, HashSet, VecDeque};

pub const DEFAULT_PCG_LOCAL_COORDINATE_ID: LocalCoordinateId = LocalCoordinateId(1);
pub const PLAYER_CHUNK_LOAD_RADIUS: f64 = 64.0;
const MAX_CHUNKS_GENERATED_PER_TICK: usize = 16;
const MAX_CHUNK_RESPONSES_PER_TICK_PER_CONNECTION: usize = 16;
const MAX_DERIVED_SVO_RESULTS_PER_TICK: usize = 16;

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
    chunk_versions: HashMap<ChunkId, UpdateVersion>,
    observation_by_player: HashMap<PlayerId, ObservationRegion>,
    subscriptions: HashMap<ConnectionId, PlayerSubscription>,
    derived_svo: DerivedSvoWorker,
}

impl Default for LocalCoordinateServerWorld {
    fn default() -> Self {
        Self {
            loaded_chunks: HashMap::new(),
            chunk_versions: HashMap::new(),
            observation_by_player: HashMap::new(),
            subscriptions: HashMap::new(),
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
