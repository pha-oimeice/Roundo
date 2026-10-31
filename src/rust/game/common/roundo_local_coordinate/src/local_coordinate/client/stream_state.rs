//! Authoritative client stream state behind the Local Coordinate IPC seam.

use super::*;
use bevy::{
    math::{DMat4, DVec3},
    prelude::IVec3,
};
use std::collections::{HashMap, HashSet};

/// Immutable authoritative SVO retained as received, without voxel-ID validation.
pub(super) struct CachedClientChunk {
    pub(super) server_version: UpdateVersion,
    pub(super) svo: Arc<VoxelChunkSvo>,
}

/// Latest payload awaiting distance-based admission to the conversion worker.
pub(super) struct PendingChunkPayload {
    pub(super) chunk: ChunkVersion,
    pub(super) payload: SerializedPayload,
}

/// Latest chunk mutation awaiting version-fenced, distance-based installation.
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
        self.advertised_server_versions.clear();
        self.pending_payloads.clear();
        self.pending_chunk_updates.clear();
        // Outstanding conversions still occupy capacity until their results are
        // collected. Their captured epoch prevents installation in this session.
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

    /// Projects existing loading anchors to Chunk coordinates in each live LC.
    /// Player/Creature motion is not a loading origin. No projected origin or
    /// per-Chunk distance is retained between scheduling calls.
    pub(super) fn loading_origins(
        &self,
        transforms: &Query<&GlobalTransform>,
    ) -> Vec<(LocalCoordinateId, ChunkCoordinate)> {
        let mut origins = Vec::new();
        for (&id, &entity) in &self.coordinates {
            let Ok(transform) = transforms.get(entity) else {
                continue;
            };
            let inverse =
                DMat4::from_cols_array(&transform.to_matrix().to_cols_array().map(f64::from))
                    .inverse();
            if !inverse.is_finite() {
                continue;
            }
            for anchor in self.rendering_anchors.values() {
                let local = inverse.transform_point3(DVec3::from_array(anchor.position));
                if local.is_finite() {
                    origins.push((
                        id,
                        local
                            .to_array()
                            .map(|axis| (axis / crate::CHUNK_EDGE_LENGTH as f64).floor() as i64),
                    ));
                }
            }
        }
        origins
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

    /// Removes every stream fact scoped to a coordinate. Worker tokens remain
    /// until completion, but can no longer publish into this coordinate lifetime.
    pub(super) fn despawn_coordinate(&mut self, id: LocalCoordinateId) -> Option<Entity> {
        self.active_server_versions
            .retain(|key, _| key.local_coordinate_id != id);
        self.advertised_server_versions
            .retain(|key, _| key.local_coordinate_id != id);
        self.cached_chunks
            .retain(|key, _| key.local_coordinate_id != id);
        self.pending_payloads.remove_coordinate(id);
        self.pending_chunk_updates.remove_coordinate(id);
        self.coordinates.remove(&id)
    }

    /// Coalesces an exactly advertised payload without starting CPU-heavy work.
    /// The same version cannot occupy pending, in-flight and installed slots more
    /// than once; initial loads and edits use exactly the same admission rule.
    pub(super) fn submit_payload(
        &mut self,
        chunk: ChunkVersion,
        payload: SerializedPayload,
    ) -> bool {
        let Some(epoch) = self.session_epoch else {
            return false;
        };
        let key = chunk.id();
        if self.advertised_server_versions.get(&key) != Some(&chunk.version)
            || self
                .cached_chunks
                .get(&key)
                .is_some_and(|cached| cached.server_version == chunk.version)
            || self
                .decode_in_flight
                .get(&key)
                .is_some_and(|&(job_epoch, version, entity)| {
                    job_epoch == epoch
                        && version == chunk.version
                        && self.coordinates.get(&key.local_coordinate_id) == Some(&entity)
                })
            || self
                .pending_payloads
                .get(key)
                .is_some_and(|pending| pending.chunk.version == chunk.version)
            || self.pending_chunk_updates.get(key).is_some_and(|pending| {
                matches!(pending, PendingChunkUpdate::Load { chunk: current, .. }
                    if current.version == chunk.version)
            })
        {
            return false;
        }
        self.pending_payloads
            .insert(key, PendingChunkPayload { chunk, payload });
        true
    }

    /// Ends active use while retaining the previously installed cache entry.
    pub(super) fn unload_chunk(
        &mut self,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    ) {
        let key = ChunkId {
            local_coordinate_id,
            coordinate,
        };
        self.active_server_versions.remove(&key);
        self.advertised_server_versions.remove(&key);
        self.pending_payloads.remove(key);
        self.queue_pending_chunk_update(PendingChunkUpdate::Unload {
            local_coordinate_id,
            coordinate,
        });
    }

    /// Coalesces the latest mutation; its reason and arrival order confer no priority.
    pub(super) fn queue_pending_chunk_update(&mut self, update: PendingChunkUpdate) {
        self.pending_chunk_updates
            .insert(pending_chunk_id(&update), update);
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

fn pending_chunk_id(update: &PendingChunkUpdate) -> ChunkId {
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
