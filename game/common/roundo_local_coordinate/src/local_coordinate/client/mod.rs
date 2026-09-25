//! Client-side coordinate lifecycle, chunk cache, and streaming IPC.

mod stream_state;
mod systems;

use stream_state::{CachedClientChunk, PendingChunkUpdate, pending_local_coordinate_id};

use crate::local_coordinate::{
    base::{LocalCoordinateBasePlugin, LocalCoordinateSet},
    data::LocalCoordinate,
    derived_svo::{DerivedSvoJob, DerivedSvoResult, DerivedSvoWorker},
    pcg::{remove_generated_chunk, replace_read_only_chunk},
    transform::LocalCoordinateTransform,
};
use crate::{
    AtomicVoxelId, AtomicVoxelRegistry, ChunkCoordinate, ChunkId, ChunkVersion, EMPTY_VOXEL_ID,
    LocalCoordinateId, LocalCoordinateIdentity, VoxelChunkSvo,
};
use avian3d::prelude::RigidBody;
use bevy::prelude::{
    App, Commands, DetectChanges, Entity, IntoScheduleConfigs, Plugin, Query, Res, ResMut,
    Resource, Update,
};
use roundo_contracts::{
    ChunkLoadingAnchorId, PredictionAnchorId, PredictionAnchorState, RenderingAnchorState,
    SerializedPayload, UpdateVersion, VirtualChunkEnvironmentOverride,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

// Per-frame budgets prevent network bursts from monopolizing the ECS update.
const MAX_COMMANDS_INGESTED_PER_FRAME: usize = 32;
const MAX_CHUNK_UPDATES_APPLIED_PER_FRAME: usize = 16;
pub const MIN_CHUNK_VIEW_DISTANCE: u16 = 1;
pub const MAX_CHUNK_VIEW_DISTANCE: u16 = 512;
pub const DEFAULT_CHUNK_VIEW_DISTANCE: u16 = 64;

#[derive(Clone, Copy, Debug, Resource)]
/// Clamped chunk radius requested from the authoritative server.
pub struct ClientChunkViewDistance(u16);

impl ClientChunkViewDistance {
    /// Creates a distance constrained to protocol-supported bounds.
    pub fn new(chunks: u16) -> Self {
        Self(chunks.clamp(MIN_CHUNK_VIEW_DISTANCE, MAX_CHUNK_VIEW_DISTANCE))
    }

    pub const fn chunks(self) -> u16 {
        self.0
    }

    /// Updates the distance while preserving protocol bounds.
    pub fn set_chunks(&mut self, chunks: u16) {
        self.0 = chunks.clamp(MIN_CHUNK_VIEW_DISTANCE, MAX_CHUNK_VIEW_DISTANCE);
    }
}

impl Default for ClientChunkViewDistance {
    fn default() -> Self {
        Self(DEFAULT_CHUNK_VIEW_DISTANCE)
    }
}

/// Client-facing endpoint for coordinate commands and chunk requests.
pub type LocalCoordinateClientIpc =
    CrossbeamThreadPipeEndpointA<LocalCoordinateClientCommand, LocalCoordinateClientEvent>;

#[derive(Clone)]
/// Installs coordinate ingestion, decoding, and ECS synchronization.
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
/// Authoritative lifecycle and chunk updates consumed by the client.
pub enum LocalCoordinateClientCommand {
    BeginSession {
        epoch: u64,
    },
    EndSession {
        epoch: u64,
    },
    RenderingAnchorSpawned(RenderingAnchorState),
    RenderingAnchorUpdated(RenderingAnchorState),
    RenderingAnchorDespawned(ChunkLoadingAnchorId),
    PredictionAnchorSpawned(PredictionAnchorState),
    PredictionAnchorUpdated(PredictionAnchorState),
    PredictionAnchorDespawned(PredictionAnchorId),
    EnvironmentOverrides(Vec<VirtualChunkEnvironmentOverride>),
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
/// Client streaming demand sent to the network bridge.
pub enum LocalCoordinateClientEvent {
    RequestChunks(Vec<ChunkId>),
    SetChunkViewDistance { chunks: u16 },
}

#[derive(Resource)]
/// Runtime indexes connecting server chunk versions to local ECS entities.
pub struct LocalCoordinateClientWorld {
    session_epoch: Option<u64>,
    coordinates: HashMap<LocalCoordinateId, Entity>,
    rendering_anchors: HashMap<ChunkLoadingAnchorId, RenderingAnchorState>,
    prediction_anchors: HashMap<PredictionAnchorId, PredictionAnchorState>,
    environment_overrides: HashMap<[i64; 3], VirtualChunkEnvironmentOverride>,
    cached_chunks: HashMap<ChunkId, CachedClientChunk>,
    active_server_versions: HashMap<ChunkId, UpdateVersion>,
    requested_server_versions: HashMap<ChunkId, UpdateVersion>,
    pending_chunk_updates: VecDeque<PendingChunkUpdate>,
    derived_svo: DerivedSvoWorker,
}

impl Default for LocalCoordinateClientWorld {
    fn default() -> Self {
        Self {
            session_epoch: None,
            coordinates: HashMap::new(),
            rendering_anchors: HashMap::new(),
            prediction_anchors: HashMap::new(),
            environment_overrides: HashMap::new(),
            cached_chunks: HashMap::new(),
            active_server_versions: HashMap::new(),
            requested_server_versions: HashMap::new(),
            pending_chunk_updates: VecDeque::new(),
            derived_svo: DerivedSvoWorker::spawn("roundo-client-derived-svo"),
        }
    }
}

impl LocalCoordinateClientWorld {
    /// Resolves the immutable environment for a prediction tick from the streamed
    /// sparse override, falling back to the protocol's complete defaults.
    pub fn environment_at(
        &self,
        position: bevy::prelude::Vec3,
    ) -> Option<(bevy::prelude::Vec3, f32, f32, u64)> {
        let coordinate =
            crate::local_coordinate::virtual_chunk::virtual_chunk_coordinate_for_position(
                position,
            )?;
        let defaults =
            crate::local_coordinate::virtual_chunk::CompleteEnvironmentDefaults::default();
        let override_ = self.environment_overrides.get(&coordinate);
        let fields = override_.map_or(0, |value| value.fields);
        let gravity = if fields & 0b001 != 0 {
            bevy::prelude::Vec3::from_array(override_.expect("field implies override").gravity)
        } else {
            defaults.gravity
        };
        let static_friction = if fields & 0b010 != 0 {
            override_.expect("field implies override").static_friction
        } else {
            defaults.static_friction
        };
        let kinetic_friction = if fields & 0b100 != 0 {
            override_.expect("field implies override").kinetic_friction
        } else {
            defaults.kinetic_friction
        };
        Some((
            gravity,
            static_friction,
            kinetic_friction,
            override_.map_or(0, |value| value.revision.0),
        ))
    }

    /// Returns the server-authoritative read-only rendering anchors.
    pub fn rendering_anchors(&self) -> impl Iterator<Item = &RenderingAnchorState> {
        self.rendering_anchors.values()
    }

    /// Returns read-only server prediction interests.
    pub fn prediction_anchors(&self) -> impl Iterator<Item = &PredictionAnchorState> {
        self.prediction_anchors.values()
    }

    /// Resolves a coordinate identity to its live ECS entity.
    pub fn entity(&self, local_coordinate_id: LocalCoordinateId) -> Option<Entity> {
        return self.coordinates.get(&local_coordinate_id).copied();
    }

    /// Returns the number of live coordinate roots.
    pub fn coordinate_count(&self) -> usize {
        self.coordinates.len()
    }

    /// Returns chunks retained for reuse across visibility changes.
    pub fn cached_chunk_count(&self) -> usize {
        self.cached_chunks.len()
    }

    /// Resolves a live ECS entity back to its coordinate identity.
    pub fn local_coordinate_id(&self, entity: Entity) -> Option<LocalCoordinateId> {
        self.coordinates
            .iter()
            .find_map(|(id, candidate)| (*candidate == entity).then_some(*id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_environment_override_is_sampled_by_position_and_revision() {
        let mut world = LocalCoordinateClientWorld::default();
        world.environment_overrides.insert(
            [-1, 0, 0],
            VirtualChunkEnvironmentOverride {
                coordinate: [-1, 0, 0],
                revision: roundo_contracts::EnvironmentRevision(9),
                fields: 0b111,
                gravity: [3.0, 0.0, 0.0],
                static_friction: 0.8,
                kinetic_friction: 0.4,
            },
        );
        assert_eq!(
            world.environment_at(bevy::prelude::Vec3::new(-0.1, 1.0, 1.0)),
            Some((bevy::prelude::Vec3::new(3.0, 0.0, 0.0), 0.8, 0.4, 9))
        );
        assert_eq!(
            world.environment_at(bevy::prelude::Vec3::new(0.1, 1.0, 1.0)),
            Some((bevy::prelude::Vec3::new(0.0, -9.81, 0.0), 0.6, 0.5, 0))
        );
    }
}
