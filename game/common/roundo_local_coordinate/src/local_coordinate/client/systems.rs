//! Client chunk-stream ingestion, validation, caching, and ECS application.

use super::*;

// The chained systems preserve command, decode, and application ordering.
impl Plugin for LocalCoordinateClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LocalCoordinateBasePlugin)
            .init_resource::<AtomicVoxelRegistry>()
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
                )
                    .chain()
                    .before(LocalCoordinateSet::RebuildIndex),
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

/// Immutable decoded SVO retained with its authoritative version.
pub(super) struct CachedClientChunk {
    server_version: UpdateVersion,
    svo: Arc<VoxelChunkSvo>,
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

        match command {
            // A session boundary invalidates every prior coordinate and version.
            LocalCoordinateClientCommand::BeginSession => {
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
                for entity in world.coordinates.drain().map(|(_, entity)| entity) {
                    commands.entity(entity).despawn();
                }
                world.cached_chunks.clear();
                world.active_server_versions.clear();
                world.requested_server_versions.clear();
                world.pending_chunk_updates.clear();
            }
            LocalCoordinateClientCommand::ResetRequests => {
                world.requested_server_versions.clear();
            }
            LocalCoordinateClientCommand::Spawn(local_coordinate_id) => {
                ensure_coordinate(&mut commands, &mut world, local_coordinate_id);
            }
            LocalCoordinateClientCommand::Despawn(local_coordinate_id) => {
                if let Some(entity) = world.coordinates.remove(&local_coordinate_id) {
                    commands.entity(entity).despawn();
                }
                world
                    .active_server_versions
                    .retain(|key, _| key.local_coordinate_id != local_coordinate_id);
                world
                    .requested_server_versions
                    .retain(|key, _| key.local_coordinate_id != local_coordinate_id);
                world
                    .pending_chunk_updates
                    .retain(|update| pending_local_coordinate_id(update) != local_coordinate_id);
            }
            LocalCoordinateClientCommand::VersionUpdates(chunks) => {
                ingest_version_updates(&mut commands, &pipe, &mut world, chunks);
            }
            // Payloads are decoded only when they match the requested version.
            LocalCoordinateClientCommand::LoadChunk { chunk, payload } => {
                let key = chunk.id();
                if world.requested_server_versions.get(&key) != Some(&chunk.version) {
                    continue;
                }
                if world
                    .derived_svo
                    .submit(DerivedSvoJob::Decode { chunk, payload })
                    .is_err()
                {
                    let removed_request = world.requested_server_versions.remove(&key);
                    debug_assert_eq!(removed_request, Some(chunk.version));
                    log::error!(
                        "cannot queue client Chunk decode: local_coordinate_id={}, coordinate={:?}, version={}, reason=derived_svo_worker_closed",
                        chunk.local_coordinate_id.0,
                        chunk.coordinate,
                        chunk.version.value()
                    );
                }
            }
            // Unload retains cached data while removing active state.
            LocalCoordinateClientCommand::UnloadChunk {
                local_coordinate_id,
                coordinate,
            } => {
                let key = ChunkId {
                    local_coordinate_id,
                    coordinate,
                };
                let removed_active = world.active_server_versions.remove(&key);
                let removed_request = world.requested_server_versions.remove(&key);
                log::trace!(
                    "queued client Chunk unload: chunk={key:?}, had_active_version={}, had_pending_request={}",
                    removed_active.is_some(),
                    removed_request.is_some()
                );
                queue_pending_chunk_update(
                    &mut world,
                    PendingChunkUpdate::Unload {
                        local_coordinate_id,
                        coordinate,
                    },
                );
            }
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
            DerivedSvoResult::Decoded { chunk, svo } => {
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
                queue_pending_chunk_update(&mut world, PendingChunkUpdate::Load { chunk, svo });
            }
            DerivedSvoResult::Failed {
                connection_id: None,
                chunk,
                error,
            } => {
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
                queue_pending_chunk_update(world, PendingChunkUpdate::Load { chunk, svo });
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

// Replaces older pending work for the same chunk identity.
fn queue_pending_chunk_update(world: &mut LocalCoordinateClientWorld, update: PendingChunkUpdate) {
    let id = pending_chunk_id(&update);
    world
        .pending_chunk_updates
        .retain(|pending| pending_chunk_id(pending) != id);
    world.pending_chunk_updates.push_back(update);
}

// Normalizes load and unload variants to one chunk key.
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

// Extracts the coordinate owner without cloning update payloads.
fn pending_local_coordinate_id(update: &PendingChunkUpdate) -> LocalCoordinateId {
    match update {
        PendingChunkUpdate::Load { chunk, .. } => chunk.local_coordinate_id,
        PendingChunkUpdate::Unload {
            local_coordinate_id,
            ..
        } => *local_coordinate_id,
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
