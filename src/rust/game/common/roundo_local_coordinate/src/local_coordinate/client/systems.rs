//! Client chunk-stream consumption, version fencing, caching, and ECS application.

use super::*;

impl Plugin for LocalCoordinateClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            LocalCoordinateReadPlugin,
            crate::local_coordinate::physics::LocalCoordinatePhysicsPlugin,
        ))
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
                admit_chunk_decodes,
                apply_pending_chunk_updates,
                reconcile_prediction_physics_interest,
            )
                .chain()
                .before(LocalCoordinateSet::RebuildIndex)
                .before(crate::local_coordinate::physics::LocalCoordinatePhysicsSet::Sync),
        )
        .add_systems(
            Update,
            discard_client_chunk_changes
                .after(LocalCoordinateSet::RebuildIndex)
                .after(crate::local_coordinate::physics::LocalCoordinatePhysicsSet::Sync),
        );
    }
}

#[derive(Resource, Clone)]
struct LocalCoordinateClientPipe(
    CrossbeamThreadPipeEndpointB<LocalCoordinateClientCommand, LocalCoordinateClientEvent>,
);

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

// Drain the finite ingress snapshot in wire order, doing no conversion or voxel
// scans here. New arrivals wait for the next snapshot, so a producer cannot keep
// this loop running indefinitely. Heavy Chunk work goes into the spatial queues.
fn ingest_commands(
    mut commands: Commands,
    pipe: Res<LocalCoordinateClientPipe>,
    distance: Res<ClientChunkViewDistance>,
    mut world: ResMut<LocalCoordinateClientWorld>,
) {
    let count = pipe.0.queued_inbound();
    for _ in 0..count {
        let Some(command) = pipe.0.try_receive() else {
            break;
        };
        match &command {
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
            LocalCoordinateClientCommand::Spawn(id) => {
                ensure_coordinate(&mut commands, &mut world, id);
            }
            LocalCoordinateClientCommand::Despawn(id) => {
                if let Some(entity) = world.despawn_coordinate(id) {
                    commands.entity(entity).despawn();
                }
            }
            LocalCoordinateClientCommand::VersionUpdates(chunks) => {
                ingest_version_updates(&mut commands, &mut world, chunks);
            }
            LocalCoordinateClientCommand::LoadChunk { chunk, payload } => {
                world.submit_payload(chunk, payload);
            }
            LocalCoordinateClientCommand::UnloadChunk {
                local_coordinate_id,
                coordinate,
            } => world.unload_chunk(local_coordinate_id, coordinate),
        }
    }
}

// Release only the exact conversion token, including obsolete session work.
// Coordinate replacement cannot turn an old result into a new installation.
fn finish_decode(world: &mut LocalCoordinateClientWorld, epoch: u64, chunk: ChunkVersion) -> bool {
    let key = chunk.id();
    let Some(&(job_epoch, version, entity)) = world.decode_in_flight.get(&key) else {
        return false;
    };
    if job_epoch != epoch || version != chunk.version {
        return false;
    }
    world.decode_in_flight.remove(&key);
    world.session_epoch == Some(epoch)
        && world.coordinates.get(&key.local_coordinate_id) == Some(&entity)
        && world.advertised_server_versions.get(&key) == Some(&chunk.version)
        && !world
            .cached_chunks
            .get(&key)
            .is_some_and(|cached| cached.server_version == chunk.version)
}

// At most the bounded outstanding conversions can return here. The client
// consumes authoritative contents without scanning or validating voxel IDs.
fn collect_derived_svo_results(mut world: ResMut<LocalCoordinateClientWorld>) {
    while let Some(result) = world.derived_svo.try_receive() {
        match result {
            DerivedSvoResult::Decoded {
                session_epoch,
                chunk,
                svo,
            } => {
                if finish_decode(&mut world, session_epoch, chunk) {
                    world.queue_pending_chunk_update(PendingChunkUpdate::Load {
                        chunk,
                        svo: Arc::new(svo),
                    });
                }
            }
            DerivedSvoResult::Failed {
                connection_id: None,
                session_epoch: Some(epoch),
                chunk,
                error,
            } => {
                if finish_decode(&mut world, epoch, chunk) {
                    panic!(
                        "cannot consume authoritative Chunk SVO: session_epoch={epoch}, chunk={chunk:?}, error={error}"
                    );
                }
            }
            DerivedSvoResult::Encoded { .. } | DerivedSvoResult::Failed { .. } => {}
        }
    }
}

// Do not hide the received set inside an unbounded worker FIFO. Only a bounded
// batch is admitted; all remaining payloads stay selectable near the live origins.
fn admit_chunk_decodes(
    transforms: Query<&GlobalTransform>,
    mut world: ResMut<LocalCoordinateClientWorld>,
) {
    let Some(epoch) = world.session_epoch else {
        return;
    };
    let origins = world.loading_origins(&transforms);
    let available =
        MAX_CHUNK_UPDATES_APPLIED_PER_FRAME.saturating_sub(world.decode_in_flight.len());
    let mut submitted = 0;
    let mut deferred = Vec::new();
    while submitted < available {
        let Some((id, pending)) = world.pending_payloads.pop_near(&origins) else {
            break;
        };
        if world.advertised_server_versions.get(&id) != Some(&pending.chunk.version) {
            continue;
        }
        if world.decode_in_flight.contains_key(&id) {
            deferred.push((id, pending));
            continue;
        }
        let Some(&entity) = world.coordinates.get(&id.local_coordinate_id) else {
            continue;
        };
        let chunk = pending.chunk;
        if world
            .derived_svo
            .submit(DerivedSvoJob::Decode {
                session_epoch: epoch,
                chunk,
                payload: pending.payload,
            })
            .is_err()
        {
            log::error!(
                "cannot queue client Chunk decode: chunk={chunk:?}, reason=derived_svo_worker_closed"
            );
            break;
        }
        world
            .decode_in_flight
            .insert(id, (epoch, chunk.version, entity));
        submitted += 1;
    }
    for (id, pending) in deferred {
        world.pending_payloads.insert(id, pending);
    }
}

// The server owns proactive payload delivery. Version announcements only update
// exact expectations or reuse an installed cache entry; they never request a
// second copy of the SVO already scheduled for delivery.
fn ingest_version_updates(
    commands: &mut Commands,
    world: &mut LocalCoordinateClientWorld,
    chunks: Vec<ChunkVersion>,
) {
    for chunk in chunks {
        let key = chunk.id();
        assert!(
            crate::local_coordinate::pcg::local_chunk_position(chunk.coordinate).is_some(),
            "cannot consume unrepresentable authoritative Chunk: chunk={chunk:?}"
        );
        ensure_coordinate(commands, world, chunk.local_coordinate_id);
        if world.advertised_server_versions.insert(key, chunk.version) == Some(chunk.version) {
            continue;
        }
        world.pending_payloads.remove(key);
        world.pending_chunk_updates.remove(key);
        if let Some(cached) = world.cached_chunks.get(&key)
            && cached.server_version == chunk.version
            && world.active_server_versions.get(&key) != Some(&chunk.version)
        {
            let svo = Arc::clone(&cached.svo);
            world.queue_pending_chunk_update(PendingChunkUpdate::Load { chunk, svo });
        }
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

// Initial loads, edits, cache hits and unloads all use the same spatial selector.
// Successfully installed authoritative payloads become active/cacheable as-is.
fn apply_pending_chunk_updates(
    transforms: Query<&GlobalTransform>,
    mut world: ResMut<LocalCoordinateClientWorld>,
    mut local_coordinates: Query<&mut LocalCoordinate>,
) {
    let origins = world.loading_origins(&transforms);
    let count = world
        .pending_chunk_updates
        .len()
        .min(MAX_CHUNK_UPDATES_APPLIED_PER_FRAME);
    let mut deferred = Vec::new();
    for _ in 0..count {
        let Some((id, update)) = world.pending_chunk_updates.pop_near(&origins) else {
            break;
        };
        let Some(&entity) = world.coordinates.get(&id.local_coordinate_id) else {
            continue;
        };
        let Ok(mut local_coordinate) = local_coordinates.get_mut(entity) else {
            deferred.push((id, update));
            continue;
        };
        match update {
            PendingChunkUpdate::Load { chunk, svo } => {
                if world.advertised_server_versions.get(&id) != Some(&chunk.version) {
                    continue;
                }
                assert!(
                    replace_read_only_chunk(
                        &mut local_coordinate,
                        chunk.coordinate,
                        Arc::clone(&svo),
                    ),
                    "cannot install authoritative Chunk: chunk={chunk:?}"
                );
                world.cached_chunks.insert(
                    id,
                    CachedClientChunk {
                        server_version: chunk.version,
                        svo,
                    },
                );
                world.active_server_versions.insert(id, chunk.version);
            }
            PendingChunkUpdate::Unload { coordinate, .. } => {
                remove_generated_chunk(&mut local_coordinate, coordinate);
            }
        }
    }
    for (id, update) in deferred {
        world.pending_chunk_updates.insert(id, update);
    }
}

fn reconcile_prediction_physics_interest(
    world: Res<LocalCoordinateClientWorld>,
    mut interests: ResMut<crate::local_coordinate::physics::LocalCoordinatePhysicsInterests>,
) {
    let desired = world.prediction_physics_interests();
    if !interests.matches(&desired) {
        interests.replace(desired);
    }
}

// Clearing derived bookkeeping must not advertise a canonical content change.
fn discard_client_chunk_changes(mut local_coordinates: Query<&mut LocalCoordinate>) {
    for mut local_coordinate in &mut local_coordinates {
        if !local_coordinate.changed_chunks.is_empty()
            || !local_coordinate.physics_dirty_chunks.is_empty()
        {
            let coordinate = local_coordinate.bypass_change_detection();
            coordinate.changed_chunks.clear();
            coordinate.physics_dirty_chunks.clear();
        }
    }
}
