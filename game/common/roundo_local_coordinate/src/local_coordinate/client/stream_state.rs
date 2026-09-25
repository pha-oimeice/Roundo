//! Authoritative client stream state behind the Local Coordinate IPC seam.

use super::*;
use bevy::prelude::IVec3;
use std::collections::{HashMap, HashSet};

/// Immutable decoded SVO retained with its authoritative version.
pub(super) struct CachedClientChunk {
    pub(super) server_version: UpdateVersion,
    pub(super) svo: Arc<VoxelChunkSvo>,
}

/// Latest-wins chunk mutation waiting for its coordinate entity.
pub(super) enum PendingChunkUpdate {
    Load {
        chunk: ChunkVersion,
        svo: Arc<VoxelChunkSvo>,
    },
    Unload {
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    },
}

impl LocalCoordinateClientWorld {
    /// Starts a new authority session and returns stale ECS roots for destruction.
    pub(super) fn begin_session(&mut self, epoch: u64) -> Vec<Entity> {
        self.session_epoch = Some(epoch);
        let stale_entities = self.coordinates.drain().map(|(_, entity)| entity).collect();
        self.rendering_anchors.clear();
        self.prediction_anchors.clear();
        self.environment_overrides.clear();
        self.cached_chunks.clear();
        self.active_server_versions.clear();
        self.requested_server_versions.clear();
        self.pending_chunk_updates.clear();
        stale_entities
    }

    pub(super) fn end_session(&mut self, epoch: u64) -> Vec<Entity> {
        if self.session_epoch != Some(epoch) {
            return Vec::new();
        }
        let stale_entities = self.begin_session(epoch);
        self.session_epoch = None;
        stale_entities
    }

    pub(super) fn session_is_active(&self) -> bool {
        self.session_epoch.is_some()
    }

    pub(super) fn spawn_rendering_anchor(&mut self, anchor: RenderingAnchorState) {
        self.rendering_anchors.insert(anchor.id, anchor);
    }

    pub(super) fn update_rendering_anchor(&mut self, anchor: RenderingAnchorState) -> bool {
        let Some(current) = self.rendering_anchors.get_mut(&anchor.id) else {
            return false;
        };
        *current = anchor;
        true
    }

    pub(super) fn despawn_rendering_anchor(&mut self, id: ChunkLoadingAnchorId) {
        self.rendering_anchors.remove(&id);
    }

    pub(super) fn spawn_prediction_anchor(&mut self, anchor: PredictionAnchorState) {
        self.prediction_anchors.insert(anchor.id, anchor);
    }

    pub(super) fn update_prediction_anchor(&mut self, anchor: PredictionAnchorState) -> bool {
        let Some(current) = self.prediction_anchors.get_mut(&anchor.id) else {
            return false;
        };
        *current = anchor;
        true
    }

    pub(super) fn despawn_prediction_anchor(&mut self, id: PredictionAnchorId) {
        self.prediction_anchors.remove(&id);
    }

    /// Applies only newer sparse environment facts.
    pub(super) fn ingest_environment_overrides(
        &mut self,
        overrides: Vec<VirtualChunkEnvironmentOverride>,
    ) {
        for override_ in overrides {
            let replace = self
                .environment_overrides
                .get(&override_.coordinate)
                .is_none_or(|current| current.revision.0 < override_.revision.0);
            if replace {
                self.environment_overrides
                    .insert(override_.coordinate, override_);
            }
        }
    }

    /// Removes one coordinate and every stream fact scoped to it.
    pub(super) fn despawn_coordinate(&mut self, id: LocalCoordinateId) -> Option<Entity> {
        self.active_server_versions
            .retain(|key, _| key.local_coordinate_id != id);
        self.requested_server_versions
            .retain(|key, _| key.local_coordinate_id != id);
        self.pending_chunk_updates
            .retain(|update| pending_local_coordinate_id(update) != id);
        self.coordinates.remove(&id)
    }

    /// Admits a payload only when it matches the outstanding authoritative version.
    pub(super) fn submit_payload(
        &mut self,
        chunk: ChunkVersion,
        payload: SerializedPayload,
    ) -> bool {
        let Some(session_epoch) = self.session_epoch else {
            return false;
        };
        let key = chunk.id();
        if self.requested_server_versions.get(&key) != Some(&chunk.version) {
            return false;
        }
        if self
            .derived_svo
            .submit(DerivedSvoJob::Decode {
                session_epoch,
                chunk,
                payload,
            })
            .is_ok()
        {
            return true;
        }
        let removed_request = self.requested_server_versions.remove(&key);
        debug_assert_eq!(removed_request, Some(chunk.version));
        log::error!(
            "cannot queue client Chunk decode: local_coordinate_id={}, coordinate={:?}, version={}, reason=derived_svo_worker_closed",
            chunk.local_coordinate_id.0,
            chunk.coordinate,
            chunk.version.value()
        );
        false
    }

    /// Ends active use of one chunk while retaining any reusable decoded cache entry.
    pub(super) fn unload_chunk(
        &mut self,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    ) {
        let key = ChunkId {
            local_coordinate_id,
            coordinate,
        };
        let removed_active = self.active_server_versions.remove(&key);
        let removed_request = self.requested_server_versions.remove(&key);
        log::trace!(
            "queued client Chunk unload: chunk={key:?}, had_active_version={}, had_pending_request={}",
            removed_active.is_some(),
            removed_request.is_some()
        );
        self.queue_pending_chunk_update(PendingChunkUpdate::Unload {
            local_coordinate_id,
            coordinate,
        });
    }

    /// Replaces older pending work for the same chunk identity.
    pub(super) fn queue_pending_chunk_update(&mut self, update: PendingChunkUpdate) {
        let id = pending_chunk_id(&update);
        self.pending_chunk_updates
            .retain(|pending| pending_chunk_id(pending) != id);
        self.pending_chunk_updates.push_back(update);
    }

    /// Derives the complete prediction-only collider interest from active stream state.
    pub(super) fn prediction_physics_interests(
        &self,
    ) -> HashMap<LocalCoordinateId, HashSet<IVec3>> {
        let mut desired = HashMap::new();
        for chunk in self.active_server_versions.keys() {
            let within_prediction = self.prediction_anchors.values().any(|anchor| {
                let center = anchor.position;
                let chunk_center = chunk.coordinate.map(|value| {
                    value as f64 * crate::CHUNK_EDGE_LENGTH as f64
                        + crate::CHUNK_EDGE_LENGTH as f64 / 2.0
                });
                let distance_squared: f64 = (0..3)
                    .map(|axis| (chunk_center[axis] - center[axis]).powi(2))
                    .sum();
                distance_squared
                    <= (f64::from(anchor.radius_chunks) * crate::CHUNK_EDGE_LENGTH as f64).powi(2)
            });
            if !within_prediction {
                continue;
            }
            let (Ok(x), Ok(y), Ok(z)) = (
                i32::try_from(chunk.coordinate[0]),
                i32::try_from(chunk.coordinate[1]),
                i32::try_from(chunk.coordinate[2]),
            ) else {
                continue;
            };
            desired
                .entry(chunk.local_coordinate_id)
                .or_insert_with(HashSet::new)
                .insert(IVec3::new(x, y, z));
        }
        desired
    }
}

pub(super) fn pending_chunk_id(update: &PendingChunkUpdate) -> ChunkId {
    match update {
        PendingChunkUpdate::Load { chunk, .. } => chunk.id(),
        PendingChunkUpdate::Unload {
            local_coordinate_id,
            coordinate,
        } => ChunkId {
            local_coordinate_id: *local_coordinate_id,
            coordinate: *coordinate,
        },
    }
}

pub(super) fn pending_local_coordinate_id(update: &PendingChunkUpdate) -> LocalCoordinateId {
    match update {
        PendingChunkUpdate::Load { chunk, .. } => chunk.local_coordinate_id,
        PendingChunkUpdate::Unload {
            local_coordinate_id,
            ..
        } => *local_coordinate_id,
    }
}
