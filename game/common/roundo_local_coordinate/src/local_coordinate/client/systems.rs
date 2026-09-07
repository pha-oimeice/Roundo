use super::*;

impl Plugin for LocalCoordinateClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LocalCoordinateBasePlugin)
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
struct LocalCoordinateClientPipe(
    CrossbeamThreadPipeEndpointB<LocalCoordinateClientCommand, LocalCoordinateClientEvent>,
);

pub(super) struct CachedClientChunk {
    server_version: UpdateVersion,
    svo: Arc<VoxelChunkSvo>,
}

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

fn publish_chunk_view_distance(
    distance: Res<ClientChunkViewDistance>,
    pipe: Res<LocalCoordinateClientPipe>,
) {
    if distance.is_changed() {
        let _ = pipe
            .0
            .try_send(LocalCoordinateClientEvent::SetChunkViewDistance {
                chunks: distance.chunks(),
            });
    }
}

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
            LocalCoordinateClientCommand::BeginSession => {
                let _ = pipe
                    .0
                    .try_send(LocalCoordinateClientEvent::SetChunkViewDistance {
                        chunks: distance.chunks(),
                    });
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
            LocalCoordinateClientCommand::LoadChunk { chunk, payload } => {
                let key = chunk.id();
                if world.requested_server_versions.get(&key) != Some(&chunk.version) {
                    continue;
                }
                world
                    .derived_svo
                    .submit(DerivedSvoJob::Decode { chunk, payload });
            }
            LocalCoordinateClientCommand::UnloadChunk {
                local_coordinate_id,
                coordinate,
            } => {
                let key = ChunkId {
                    local_coordinate_id,
                    coordinate,
                };
                world.active_server_versions.remove(&key);
                world.requested_server_versions.remove(&key);
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

fn collect_derived_svo_results(
    mut commands: Commands,
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
                world.requested_server_versions.remove(&key);
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
                world.requested_server_versions.remove(&chunk.id());
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
            world.requested_server_versions.remove(&key);
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
        let _ = pipe
            .0
            .try_send(LocalCoordinateClientEvent::RequestChunks(requests));
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
                    LocalCoordinateIdentity(local_coordinate_id),
                    LocalCoordinate::default(),
                    RigidBody::Static,
                    LocalCoordinateTransform::default(),
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
            PendingChunkUpdate::Load { chunk, svo } => {
                replace_read_only_chunk(&mut local_coordinate, chunk.coordinate, svo);
            }
            PendingChunkUpdate::Unload { coordinate, .. } => {
                remove_generated_chunk(&mut local_coordinate, coordinate);
            }
        }
    }
}

fn queue_pending_chunk_update(world: &mut LocalCoordinateClientWorld, update: PendingChunkUpdate) {
    let id = pending_chunk_id(&update);
    world
        .pending_chunk_updates
        .retain(|pending| pending_chunk_id(pending) != id);
    world.pending_chunk_updates.push_back(update);
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

fn pending_local_coordinate_id(update: &PendingChunkUpdate) -> LocalCoordinateId {
    match update {
        PendingChunkUpdate::Load { chunk, .. } => chunk.local_coordinate_id,
        PendingChunkUpdate::Unload {
            local_coordinate_id,
            ..
        } => *local_coordinate_id,
    }
}

fn discard_client_chunk_changes(mut local_coordinates: Query<&mut LocalCoordinate>) {
    for mut local_coordinate in &mut local_coordinates {
        local_coordinate.changed_chunks.clear();
        local_coordinate.physics_dirty_chunks.clear();
    }
}

#[cfg(test)]
mod tests;
