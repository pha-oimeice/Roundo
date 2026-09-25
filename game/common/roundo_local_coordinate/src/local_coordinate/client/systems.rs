//! Client chunk-stream ingestion, validation, caching, and ECS application.

use super::*;

// The chained systems preserve command, decode, and application ordering.
impl Plugin for LocalCoordinateClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            LocalCoordinateBasePlugin,
            crate::local_coordinate::physics::LocalCoordinatePhysicsPlugin,
        ))
        .init_resource::<AtomicVoxelRegistry>()
        // Physics is prediction-only on clients; begin restricted before the
        // first rendering chunk can reach the collider sync system.
        .insert_resource(
            crate::local_coordinate::physics::LocalCoordinatePhysicsInterests::restricted_empty(),
        )
        .init_resource::<LocalCoordinateClientWorld>()
        .init_resource::<ClientChunkViewDistance>()
        .insert_resource(LocalCoordinateClientPipe(self.pipe.endpoint_b()))
        .add_systems(
            Update,
            (
                publish_chunk_view_distance,
                ingest_commands,
                collect_derived_svo_results,
                apply_pending_chunk_updates,
                reconcile_prediction_physics_interest,
            )
                .chain()
                .before(LocalCoordinateSet::RebuildIndex)
                .before(crate::local_coordinate::physics::LocalCoordinatePhysicsSet::Sync),
        )
        .add_systems(
            Update,
            discard_client_chunk_changes.after(LocalCoordinateSet::RebuildIndex),
        );
    }
}

#[derive(Resource, Clone)]
/// ECS-side endpoint for coordinate commands and client requests.
struct LocalCoordinateClientPipe(
    CrossbeamThreadPipeEndpointB<LocalCoordinateClientCommand, LocalCoordinateClientEvent>,
);

// Publishes only changed, already-clamped view distances.
fn publish_chunk_view_distance(
    distance: Res<ClientChunkViewDistance>,
    pipe: Res<LocalCoordinateClientPipe>,
) {
    if distance.is_changed() {
        let chunks = distance.chunks();
        if pipe
            .0
            .try_send(LocalCoordinateClientEvent::SetChunkViewDistance { chunks })
            .is_err()
        {
            log::error!(
                "cannot publish client Chunk view distance: chunks={chunks}, reason=client_bridge_closed"
            );
        }
    }
}

// Applies a bounded batch of lifecycle and chunk transport commands.
fn ingest_commands(
    mut commands: Commands,
    pipe: Res<LocalCoordinateClientPipe>,
    distance: Res<ClientChunkViewDistance>,
    mut world: ResMut<LocalCoordinateClientWorld>,
) {
    for _ in 0..MAX_COMMANDS_INGESTED_PER_FRAME {
        let Some(command) = pipe.0.try_receive() else {
            break;
        };

        match &command {
            // A session boundary invalidates every prior coordinate and version.
            LocalCoordinateClientCommand::BeginSession { epoch } => {
                let chunks = distance.chunks();
                if pipe
                    .0
                    .try_send(LocalCoordinateClientEvent::SetChunkViewDistance { chunks })
                    .is_err()
                {
                    log::error!(
                        "cannot initialize client Chunk view distance: chunks={chunks}, reason=client_bridge_closed"
                    );
                }
                for entity in world.begin_session(*epoch) {
                    commands.entity(entity).despawn();
                }
                continue;
            }
            LocalCoordinateClientCommand::EndSession { epoch } => {
                for entity in world.end_session(*epoch) {
                    commands.entity(entity).despawn();
                }
                continue;
            }
            _ if !world.session_is_active() => continue,
            _ => {}
        }

        match command {
            LocalCoordinateClientCommand::BeginSession { .. }
            | LocalCoordinateClientCommand::EndSession { .. } => unreachable!(),
            LocalCoordinateClientCommand::RenderingAnchorSpawned(anchor) => {
                world.spawn_rendering_anchor(anchor);
            }
            LocalCoordinateClientCommand::RenderingAnchorUpdated(anchor) => {
                if !world.update_rendering_anchor(anchor) {
                    log::warn!(
                        "received update for unknown rendering anchor: anchor_id={}",
                        anchor.id.0
                    );
                }
            }
            LocalCoordinateClientCommand::RenderingAnchorDespawned(anchor_id) => {
                world.despawn_rendering_anchor(anchor_id);
            }
            LocalCoordinateClientCommand::PredictionAnchorSpawned(anchor) => {
                world.spawn_prediction_anchor(anchor);
            }
            LocalCoordinateClientCommand::PredictionAnchorUpdated(anchor) => {
                world.update_prediction_anchor(anchor);
            }
            LocalCoordinateClientCommand::PredictionAnchorDespawned(anchor_id) => {
                world.despawn_prediction_anchor(anchor_id);
            }
            LocalCoordinateClientCommand::EnvironmentOverrides(overrides) => {
                world.ingest_environment_overrides(overrides);
            }
            LocalCoordinateClientCommand::Spawn(local_coordinate_id) => {
                ensure_coordinate(&mut commands, &mut world, local_coordinate_id);
            }
            LocalCoordinateClientCommand::Despawn(local_coordinate_id) => {
                if let Some(entity) = world.despawn_coordinate(local_coordinate_id) {
                    commands.entity(entity).despawn();
                }
            }
            LocalCoordinateClientCommand::VersionUpdates(chunks) => {
                ingest_version_updates(&mut commands, &pipe, &mut world, chunks);
            }
            // Payloads are decoded only when they match the requested version.
            LocalCoordinateClientCommand::LoadChunk { chunk, payload } => {
                world.submit_payload(chunk, payload);
            }
            // Unload retains cached data while removing active state.
            LocalCoordinateClientCommand::UnloadChunk {
                local_coordinate_id,
                coordinate,
            } => world.unload_chunk(local_coordinate_id, coordinate),
        }
    }
}

// Validates decoded voxel IDs before publishing chunks to ECS state.
fn collect_derived_svo_results(
    mut commands: Commands,
    voxels: Option<Res<AtomicVoxelRegistry>>,
    mut world: ResMut<LocalCoordinateClientWorld>,
) {
    for _ in 0..MAX_CHUNK_UPDATES_APPLIED_PER_FRAME {
        let Some(result) = world.derived_svo.try_receive() else {
            break;
        };
        match result {
            DerivedSvoResult::Decoded {
                session_epoch,
                chunk,
                svo,
            } => {
                if world.session_epoch != Some(session_epoch) {
                    continue;
                }
                let key = chunk.id();
                if world.requested_server_versions.get(&key) != Some(&chunk.version) {
                    continue;
                }
                let removed_request = world.requested_server_versions.remove(&key);
                debug_assert_eq!(removed_request, Some(chunk.version));
                let builtin;
                let voxels = if let Some(voxels) = voxels.as_deref() {
                    voxels
                } else {
                    builtin = AtomicVoxelRegistry::builtin();
                    &builtin
                };
                if let Some(unknown) = first_unknown_voxel(&svo, voxels) {
                    log::error!(
                        "discarding Chunk {:?}: server sent unknown atomic voxel ID {}",
                        key,
                        unknown.0
                    );
                    continue;
                }
                let svo = Arc::new(svo);
                world.cached_chunks.insert(
                    key,
                    CachedClientChunk {
                        server_version: chunk.version,
                        svo: Arc::clone(&svo),
                    },
                );
                world.active_server_versions.insert(key, chunk.version);
                ensure_coordinate(&mut commands, &mut world, chunk.local_coordinate_id);
                world.queue_pending_chunk_update(PendingChunkUpdate::Load { chunk, svo });
            }
            DerivedSvoResult::Failed {
                connection_id: None,
                session_epoch: Some(session_epoch),
                chunk,
                error,
            } => {
                if world.session_epoch != Some(session_epoch) {
                    continue;
                }
                let removed_request = world.requested_server_versions.remove(&chunk.id());
                if removed_request != Some(chunk.version) {
                    log::debug!(
                        "received failure for inactive client Chunk derivation: chunk={:?}, failed_version={}",
                        chunk.id(),
                        chunk.version.value()
                    );
                }
                log::warn!(
                    "failed to derive client chunk SVO: local_coordinate_id={}, coordinate={:?}, error={error}",
                    chunk.local_coordinate_id.0,
                    chunk.coordinate
                );
            }
            DerivedSvoResult::Encoded { .. }
            | DerivedSvoResult::Failed {
                connection_id: Some(_),
                ..
            }
            | DerivedSvoResult::Failed {
                connection_id: None,
                session_epoch: None,
                ..
            } => {}
        }
    }
}

// Reuses matching cache entries and requests only unknown versions.
fn ingest_version_updates(
    commands: &mut Commands,
    pipe: &LocalCoordinateClientPipe,
    world: &mut LocalCoordinateClientWorld,
    chunks: Vec<ChunkVersion>,
) {
    let mut requests = Vec::new();
    for chunk in chunks {
        let key = chunk.id();
        ensure_coordinate(commands, world, chunk.local_coordinate_id);
        if let Some(cached) = world.cached_chunks.get(&key)
            && cached.server_version == chunk.version
        {
            let superseded_request = world.requested_server_versions.remove(&key);
            if let Some(version) = superseded_request
                && version != chunk.version
            {
                log::trace!(
                    "cancelled obsolete client Chunk request after cache hit: chunk={key:?}, requested_version={}, cached_version={}",
                    version.value(),
                    chunk.version.value()
                );
            }
            if world.active_server_versions.get(&key) != Some(&chunk.version) {
                world.active_server_versions.insert(key, chunk.version);
                let svo = Arc::clone(&cached.svo);
                world.queue_pending_chunk_update(PendingChunkUpdate::Load { chunk, svo });
            }
            continue;
        }
        if world.requested_server_versions.get(&key) != Some(&chunk.version) {
            world.requested_server_versions.insert(key, chunk.version);
            requests.push(key);
        }
    }

    if !requests.is_empty() {
        let request_count = requests.len();
        if pipe
            .0
            .try_send(LocalCoordinateClientEvent::RequestChunks(requests))
            .is_err()
        {
            log::error!(
                "cannot publish client Chunk requests: chunk_count={request_count}, reason=client_bridge_closed"
            );
        }
    }
}

// Lazily creates a static coordinate root for incoming chunks.
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
                    LocalCoordinateIdentity(local_coordinate_id),
                    LocalCoordinate::default(),
                    RigidBody::Static,
                    LocalCoordinateTransform::default(),
                ))
                .id()
        })
}

// Rejects nonempty voxel IDs absent from the loaded resource registry.
fn first_unknown_voxel(
    svo: &roundo_algorithm::tree::BreadthFirstLosslessSvo<AtomicVoxelId>,
    voxels: &AtomicVoxelRegistry,
) -> Option<AtomicVoxelId> {
    let edge = crate::local_coordinate::data::CHUNK_EDGE_LENGTH as u32;
    for z in 0..edge {
        for y in 0..edge {
            for x in 0..edge {
                let voxel = *svo
                    .value_at_coordinates([x, y, z])
                    .expect("decoded Chunk SVO has the fixed local-coordinate depth");
                if voxel != EMPTY_VOXEL_ID && voxels.definition(voxel).is_none() {
                    return Some(voxel);
                }
            }
        }
    }
    None
}

// Applies a bounded update batch; unavailable entities are retried later.
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
            PendingChunkUpdate::Load { chunk, svo } => {
                replace_read_only_chunk(&mut local_coordinate, chunk.coordinate, svo);
            }
            PendingChunkUpdate::Unload { coordinate, .. } => {
                remove_generated_chunk(&mut local_coordinate, coordinate);
            }
        }
    }
}

/// Keeps collider derivation limited to loaded prediction-interest chunks;
/// rendering data stays cached independently.
fn reconcile_prediction_physics_interest(
    world: Res<LocalCoordinateClientWorld>,
    mut interests: ResMut<crate::local_coordinate::physics::LocalCoordinatePhysicsInterests>,
) {
    let desired = world.prediction_physics_interests();
    if !interests.matches(&desired) {
        interests.replace(desired);
    }
}

// Server-derived chunks are read-only locally, so transient dirty sets are cleared.
fn discard_client_chunk_changes(mut local_coordinates: Query<&mut LocalCoordinate>) {
    for mut local_coordinate in &mut local_coordinates {
        local_coordinate.changed_chunks.clear();
        local_coordinate.physics_dirty_chunks.clear();
    }
}

#[cfg(test)]
mod tests;
