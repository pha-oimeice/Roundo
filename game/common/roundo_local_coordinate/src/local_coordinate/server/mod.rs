//! Authoritative procedural-coordinate streaming and network-facing IPC contracts.

mod generation;
mod stream_state;
mod systems;

use generation::{ChunkGenerationJob, ChunkGenerationWorker};
use stream_state::PlayerSubscription;
use systems::{GenerationPlan, ObservationRegion};

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
    Resource, Startup, SystemSet, Update,
};
use roundo_contracts::{
    ChunkLoadingAnchorId, ConnectionId, PlayerId, PredictionAnchorId, PredictionAnchorState,
    RenderingAnchorState, SceneId, SerializedPayload, UpdateVersion,
    VirtualChunkEnvironmentOverride,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet, VecDeque};

/// Identity assigned to the plugin's default generated coordinate.
pub const DEFAULT_PCG_LOCAL_COORDINATE_ID: LocalCoordinateId = LocalCoordinateId(1);
const MAX_CHUNKS_GENERATED_PER_TICK: usize = 16;
const MAX_CHUNK_GENERATION_JOBS_IN_FLIGHT: usize = 32;
const MAX_CHUNKS_EVICTED_PER_TICK: usize = 32;
/// Dummy radius for the independent controlled-Creature prediction interest.
pub const PREDICTION_CHUNK_RADIUS: u16 = 4;
const MAX_CHUNK_RESPONSES_PER_TICK_PER_CONNECTION: usize = 16;
const MAX_DERIVED_SVO_RESULTS_PER_TICK: usize = 16;
const MAX_DERIVED_SVO_JOBS_IN_FLIGHT: usize = 32;

/// Variable-update ordering boundaries for server streaming work.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum LocalCoordinateServerSet {
    /// Consumes commands and worker generation results before index rebuilding.
    Prepare,
    /// Advertises versions and publishes derived payloads after index rebuilding.
    Commit,
}

/// Clonable network-facing endpoint for coordinate commands and events.
///
/// Clones share the plugin's two unbounded in-process queues. Sending is
/// non-blocking and does not wait for command application or payload generation.
pub type LocalCoordinateServerIpc =
    CrossbeamThreadPipeEndpointA<LocalCoordinateServerCommand, LocalCoordinateServerEvent>;

/// Installs generated coordinates, bounded worker pipelines, and streaming IPC.
///
/// Cloning shares the IPC queues but copies generator configuration.
#[derive(Clone)]
pub struct LocalCoordinateServerPlugin {
    pipe: CrossbeamThreadPipe<LocalCoordinateServerCommand, LocalCoordinateServerEvent>,
    generated_coordinates: Vec<PcgLocalCoordinate>,
}

impl LocalCoordinateServerPlugin {
    /// Creates a plugin with one scene-S1 superflat coordinate at height zero.
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                SuperflatGenerator::new(0),
            )],
        }
    }

    /// Creates a plugin whose only generated coordinate uses `generator`.
    pub fn with_generator(generator: SuperflatGenerator) -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                generator,
            )],
        }
    }

    /// Adds or replaces a generated coordinate configuration by identity.
    ///
    /// Replacement preserves the existing coordinate's scene selection; a new
    /// entry defaults to [`SceneId::S1`]. This only configures future app setup.
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

    /// Returns an endpoint sharing this plugin's command and event queues.
    pub fn ipc(&self) -> LocalCoordinateServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for LocalCoordinateServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Marks an authoritative coordinate backed by deterministic procedural generation.
#[derive(Component, Clone, Copy, Debug)]
pub struct PcgLocalCoordinate {
    pub id: LocalCoordinateId,
    pub generator: SuperflatGenerator,
    pub scene_id: SceneId,
}

impl PcgLocalCoordinate {
    /// Creates a generated coordinate in [`SceneId::S1`].
    pub const fn new(id: LocalCoordinateId, generator: SuperflatGenerator) -> Self {
        Self {
            id,
            generator,
            scene_id: SceneId::S1,
        }
    }

    /// Assigns the scene whose observers may request this coordinate.
    pub const fn in_scene(mut self, scene_id: SceneId) -> Self {
        self.scene_id = scene_id;
        self
    }
}

/// Connection-scoped demand submitted by the network adapter.
#[derive(Clone, Debug)]
pub enum LocalCoordinateServerCommand {
    /// Starts or replaces a connection's subscription for one player identity.
    SubscribePlayer {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    /// Removes subscription, requested distance, and in-flight derived jobs.
    UnsubscribePlayer { connection_id: ConnectionId },
    /// Requests payloads for versions previously advertised to this connection.
    RequestChunks {
        connection_id: ConnectionId,
        chunks: Vec<ChunkId>,
    },
    /// Stores a clamped chunk radius, including before subscription exists.
    SetChunkViewDistance {
        connection_id: ConnectionId,
        chunks: u16,
    },
    /// Updates separate server-owned anchors from a controlled Creature pose.
    UpdateControlledCreaturePosition {
        connection_id: ConnectionId,
        position: [f64; 3],
    },
}

/// Connection-addressed lifecycle, version, and payload output.
#[derive(Clone, Debug)]
pub enum LocalCoordinateServerEvent {
    RenderingAnchorSpawned {
        connection_id: ConnectionId,
        anchor: RenderingAnchorState,
    },
    RenderingAnchorUpdated {
        connection_id: ConnectionId,
        anchor: RenderingAnchorState,
    },
    RenderingAnchorDespawned {
        connection_id: ConnectionId,
        anchor_id: ChunkLoadingAnchorId,
    },
    PredictionAnchorSpawned {
        connection_id: ConnectionId,
        anchor: PredictionAnchorState,
    },
    PredictionAnchorUpdated {
        connection_id: ConnectionId,
        anchor: PredictionAnchorState,
    },
    PredictionAnchorDespawned {
        connection_id: ConnectionId,
        anchor_id: PredictionAnchorId,
    },
    EnvironmentOverrides {
        connection_id: ConnectionId,
        overrides: Vec<VirtualChunkEnvironmentOverride>,
    },
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

/// Server-owned generation, subscription, version, and worker bookkeeping.
///
/// This resource is observable for diagnostics; mutation is owned by the
/// streaming systems so its indexes remain mutually consistent.
#[derive(Resource)]
pub struct LocalCoordinateServerWorld {
    loaded_chunks: HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
    generation_plans: HashMap<LocalCoordinateId, GenerationPlan>,
    generation_jobs: HashSet<ChunkId>,
    generation_worker: ChunkGenerationWorker,
    chunk_versions: HashMap<ChunkId, UpdateVersion>,
    observation_by_anchor: HashMap<ChunkLoadingAnchorId, ObservationRegion>,
    subscriptions: HashMap<ConnectionId, PlayerSubscription>,
    next_anchor_id: u64,
    next_prediction_anchor_id: u64,
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
            observation_by_anchor: HashMap::new(),
            subscriptions: HashMap::new(),
            next_anchor_id: 1,
            next_prediction_anchor_id: 1,
            requested_view_distances: HashMap::new(),
            derived_svo_jobs: HashMap::new(),
            derived_svo: DerivedSvoWorker::spawn("roundo-server-derived-svo"),
        }
    }
}

impl LocalCoordinateServerWorld {
    /// Borrows a retained generated chunk, or returns `None` if not loaded.
    pub fn chunk(
        &self,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    ) -> Option<&GeneratedChunk> {
        let chunks = self.loaded_chunks.get(&local_coordinate_id)?;
        return chunks.get(&coordinate);
    }

    /// Returns the number of retained generated chunks for one coordinate.
    pub fn loaded_chunk_count(&self, local_coordinate_id: LocalCoordinateId) -> usize {
        let chunks = self.loaded_chunks.get(&local_coordinate_id);
        chunks.map_or(0, HashMap::len)
    }

    /// Returns the number of coordinate IDs with generation bookkeeping entries.
    ///
    /// An entry may currently contain zero loaded chunks.
    pub fn coordinate_count(&self) -> usize {
        self.loaded_chunks.len()
    }
}
