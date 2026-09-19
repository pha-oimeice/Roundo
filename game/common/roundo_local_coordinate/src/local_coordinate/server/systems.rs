//! Server-side observation, procedural generation, version advertisement, and SVO delivery.
//!
//! The update pipeline first consumes subscription state and commits bounded PCG
//! results, then rebuilds spatial indexes, advances authoritative chunk versions,
//! advertises desired versions, and serves explicit payload requests. Generation
//! and SVO conversion run on workers; their results are revalidated before commit.

use super::*;

impl Plugin for LocalCoordinateServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LocalCoordinateBasePlugin, LocalCoordinatePhysicsPlugin))
            .init_resource::<crate::AtomicVoxelRegistry>()
            .insert_resource(GeneratedLocalCoordinates(
                self.generated_coordinates.clone(),
            ))
            .init_resource::<LocalCoordinateServerWorld>()
            .insert_resource(LocalCoordinateServerPipe(self.pipe.endpoint_b()))
            .configure_sets(
                Update,
                (
                    LocalCoordinateServerSet::Prepare,
                    LocalCoordinateSet::RebuildIndex,
                    LocalCoordinateServerSet::Commit,
                    LocalCoordinatePhysicsSet::Sync,
                )
                    .chain(),
            )
            .add_systems(Startup, spawn_generated_local_coordinates)
            // World streaming has its own variable-rate tick. The authoritative
            // movement/interaction simulation remains in FixedUpdate and never
            // waits for PCG, subscription scans, or SVO publication.
            .add_systems(
                Update,
                (prepare_player_chunks, commit_generated_chunks)
                    .chain()
                    .in_set(LocalCoordinateServerSet::Prepare),
            )
            .add_systems(
                Update,
                (
                    update_authoritative_chunk_versions,
                    publish_subscription_changes,
                    serve_requested_chunks,
                    publish_derived_svo_results,
                )
                    .chain()
                    .in_set(LocalCoordinateServerSet::Commit),
            );
    }
}

#[derive(Resource, Clone)]
struct GeneratedLocalCoordinates(Vec<PcgLocalCoordinate>);

#[derive(Resource, Clone)]
struct LocalCoordinateServerPipe(
    CrossbeamThreadPipeEndpointB<LocalCoordinateServerCommand, LocalCoordinateServerEvent>,
);

/// Per-connection view and the exact state last advertised to that connection.
pub(super) struct PlayerSubscription {
    anchor: RenderingAnchorState,
    spawned_coordinates: HashSet<LocalCoordinateId>,
    advertised_chunks: HashMap<ChunkId, UpdateVersion>,
    pending_requests: VecDeque<ChunkId>,
}

/// Spherical observation region expressed in absolute or coordinate-local units.
#[derive(Clone, Copy)]
pub(super) struct ObservationRegion {
    /// Exact observer position, retained for physics and nearest-first ordering.
    center: [f64; 3],
    /// Center of the hysteresis-stabilized Chunk used by streaming membership.
    streaming_center: [f64; 3],
    radius: f64,
    scene_id: SceneId,
}

/// Incremental generation/eviction plan for one procedural coordinate.
///
/// `signature` suppresses replanning while exact anchor positions and radii are
/// unchanged. Pending work is nearest-first at plan creation; later queue order
/// is retained until the signature changes.
#[derive(Default)]
pub(super) struct GenerationPlan {
    signature: Vec<([u64; 3], u16)>,
    desired: HashSet<ChunkCoordinate>,
    pending: VecDeque<ChunkCoordinate>,
    evicting: VecDeque<ChunkCoordinate>,
}

// Creates one static authoritative coordinate root per configured generator.
fn spawn_generated_local_coordinates(
    mut commands: Commands,
    coordinates: Res<GeneratedLocalCoordinates>,
) {
    for coordinate in &coordinates.0 {
        commands.spawn((
            LocalCoordinate::default(),
            LocalCoordinateIdentity(coordinate.id),
            *coordinate,
            RigidBody::Static,
            LocalCoordinateTransform::default(),
        ));
    }
}

// Reconciles commands and observations, then admits bounded nearest-first PCG work.
fn prepare_player_chunks(
    pipe: Res<LocalCoordinateServerPipe>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut physics_interests: ResMut<LocalCoordinatePhysicsInterests>,
    mut local_coordinates: Query<(&PcgLocalCoordinate, &GlobalTransform, &mut LocalCoordinate)>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            LocalCoordinateServerCommand::SubscribePlayer {
                connection_id,
                player_id,
            } => {
                let radius_chunks = world
                    .requested_view_distances
                    .get(&connection_id)
                    .copied()
                    .unwrap_or(DEFAULT_CHUNK_VIEW_DISTANCE);
                let anchor = RenderingAnchorState {
                    id: ChunkLoadingAnchorId(world.next_anchor_id),
                    owner: player_id,
                    scene_id: SceneId::S1,
                    position: [0.0; 3],
                    radius_chunks,
                };
                world.next_anchor_id = world.next_anchor_id.saturating_add(1);
                world.subscriptions.insert(
                    connection_id,
                    PlayerSubscription {
                        anchor,
                        spawned_coordinates: HashSet::new(),
                        advertised_chunks: HashMap::new(),
                        pending_requests: VecDeque::new(),
                    },
                );
                if pipe
                    .0
                    .try_send(LocalCoordinateServerEvent::RenderingAnchorSpawned {
                        connection_id,
                        anchor,
                    })
                    .is_err()
                {
                    log::error!(
                        "cannot publish rendering anchor spawn: connection_id={}, anchor_id={}, reason=server_bridge_closed",
                        connection_id.0,
                        anchor.id.0
                    );
                }
            }
            LocalCoordinateServerCommand::UnsubscribePlayer { connection_id } => {
                let removed_subscription = world.subscriptions.remove(&connection_id);
                let removed_distance = world.requested_view_distances.remove(&connection_id);
                if let Some(subscription) = removed_subscription {
                    if pipe
                        .0
                        .try_send(LocalCoordinateServerEvent::RenderingAnchorDespawned {
                            connection_id,
                            anchor_id: subscription.anchor.id,
                        })
                        .is_err()
                    {
                        log::error!(
                            "cannot publish rendering anchor despawn: connection_id={}, anchor_id={}, reason=server_bridge_closed",
                            connection_id.0,
                            subscription.anchor.id.0
                        );
                    }
                } else {
                    log::debug!(
                        "ignored unsubscribe for unknown Local Coordinate subscription: connection_id={}, had_view_distance={}",
                        connection_id.0,
                        removed_distance.is_some()
                    );
                }
                world
                    .derived_svo_jobs
                    .retain(|(candidate, _), _| *candidate != connection_id);
            }
            LocalCoordinateServerCommand::RequestChunks {
                connection_id,
                chunks,
            } => {
                let Some(subscription) = world.subscriptions.get_mut(&connection_id) else {
                    continue;
                };
                enqueue_chunk_requests(subscription, chunks);
            }
            LocalCoordinateServerCommand::SetChunkViewDistance {
                connection_id,
                chunks,
            } => {
                let chunks = chunks.clamp(MIN_CHUNK_VIEW_DISTANCE, MAX_CHUNK_VIEW_DISTANCE);
                world.requested_view_distances.insert(connection_id, chunks);
                if let Some(subscription) = world.subscriptions.get_mut(&connection_id) {
                    subscription.anchor.radius_chunks = chunks;
                    let anchor = subscription.anchor;
                    if pipe
                        .0
                        .try_send(LocalCoordinateServerEvent::RenderingAnchorUpdated {
                            connection_id,
                            anchor,
                        })
                        .is_err()
                    {
                        log::error!(
                            "cannot publish rendering anchor update: connection_id={}, anchor_id={}, reason=server_bridge_closed",
                            connection_id.0,
                            anchor.id.0
                        );
                    }
                }
            }
        }
    }

    world.observation_by_anchor = world
        .subscriptions
        .values()
        .map(|subscription| {
            let anchor = subscription.anchor;
            (
                anchor.id,
                ObservationRegion {
                    center: anchor.position,
                    streaming_center: anchor.position,
                    radius: f64::from(anchor.radius_chunks) * CHUNK_EDGE_LENGTH as f64,
                    scene_id: anchor.scene_id,
                },
            )
        })
        .collect();

    let mut active_coordinates = HashSet::new();
    let mut desired_physics = HashMap::new();
    let mut generation_budget = MAX_CHUNKS_GENERATED_PER_TICK
        .min(MAX_CHUNK_GENERATION_JOBS_IN_FLIGHT.saturating_sub(world.generation_jobs.len()));
    let generation_worker = world.generation_worker.clone();

    for (generated, coordinate_transform, mut local_coordinate) in &mut local_coordinates {
        active_coordinates.insert(generated.id);
        let observations = world
            .observation_by_anchor
            .values()
            .filter(|observation| observation.scene_id == generated.scene_id)
            .filter_map(|observation| local_observation_region(coordinate_transform, *observation))
            .collect::<Vec<_>>();
        desired_physics.insert(
            generated.id,
            observations
                .iter()
                .flat_map(|observation| {
                    superflat_chunks_intersecting_radius(
                        observation.center,
                        PHYSICS_CHUNK_RADIUS * CHUNK_EDGE_LENGTH as f64,
                        generated.generator.height,
                    )
                })
                .filter_map(crate::local_coordinate::pcg::local_chunk_position)
                .collect(),
        );
        let mut signature = observations
            .iter()
            .map(|observation| {
                (
                    observation.streaming_center.map(f64::to_bits),
                    (observation.radius / CHUNK_EDGE_LENGTH as f64).round() as u16,
                )
            })
            .collect::<Vec<_>>();
        signature.sort_unstable();
        let LocalCoordinateServerWorld {
            loaded_chunks,
            generation_plans,
            generation_jobs,
            chunk_versions,
            ..
        } = &mut *world;
        let loaded_chunks = loaded_chunks.entry(generated.id).or_default();
        let plan = generation_plans.entry(generated.id).or_default();
        if plan.signature != signature {
            let snapped_observations = signature
                .iter()
                .map(|(center, distance)| ObservationRegion {
                    center: center.map(f64::from_bits),
                    streaming_center: center.map(f64::from_bits),
                    radius: f64::from(*distance) * CHUNK_EDGE_LENGTH as f64,
                    scene_id: generated.scene_id,
                })
                .collect::<Vec<_>>();
            let desired_chunks = snapped_observations
                .iter()
                .flat_map(|observation| {
                    superflat_chunks_intersecting_radius(
                        observation.center,
                        observation.radius,
                        generated.generator.height,
                    )
                })
                .collect::<HashSet<_>>();
            let evictable = loaded_chunks
                .keys()
                .filter(|coordinate| !desired_chunks.contains(*coordinate))
                .filter(|coordinate| {
                    let id = ChunkId {
                        local_coordinate_id: generated.id,
                        coordinate: **coordinate,
                    };
                    chunk_versions.get(&id) == Some(&UpdateVersion::INITIAL)
                        && crate::local_coordinate::pcg::local_chunk_position(**coordinate)
                            .is_none_or(|position| {
                                !local_coordinate.changed_chunks.contains(&position)
                            })
                })
                .copied()
                .collect::<Vec<_>>();
            plan.evicting = evictable.into();

            let mut missing = desired_chunks
                .iter()
                .filter(|coordinate| !loaded_chunks.contains_key(*coordinate))
                .copied()
                .collect::<Vec<_>>();
            missing.sort_by(|left, right| {
                nearest_chunk_distance_squared(*left, &snapped_observations)
                    .total_cmp(&nearest_chunk_distance_squared(
                        *right,
                        &snapped_observations,
                    ))
                    .then_with(|| left.cmp(right))
            });
            plan.signature = signature;
            plan.desired = desired_chunks;
            plan.pending = missing.into();
        }
        for coordinate in plan
            .evicting
            .drain(..plan.evicting.len().min(MAX_CHUNKS_EVICTED_PER_TICK))
        {
            let removed_chunk = loaded_chunks.remove(&coordinate);
            debug_assert!(
                removed_chunk.is_some(),
                "planned Chunk eviction must be loaded"
            );
            remove_generated_chunk(&mut local_coordinate, coordinate);
        }
        while generation_budget > 0 {
            let Some(coordinate) = plan.pending.pop_front() else {
                break;
            };
            let id = ChunkId {
                local_coordinate_id: generated.id,
                coordinate,
            };
            if !generation_jobs.insert(id) {
                continue;
            }
            if generation_worker
                .submit(ChunkGenerationJob {
                    local_coordinate_id: generated.id,
                    coordinate,
                    generator: generated.generator,
                })
                .is_err()
            {
                let removed_job = generation_jobs.remove(&id);
                debug_assert!(removed_job);
                plan.pending.push_front(coordinate);
                log::error!(
                    "cannot queue Chunk generation: local_coordinate_id={}, coordinate={coordinate:?}, reason=generation_worker_closed",
                    generated.id.0
                );
                break;
            }
            generation_budget -= 1;
        }
    }

    world
        .loaded_chunks
        .retain(|local_coordinate_id, _| active_coordinates.contains(local_coordinate_id));
    world
        .generation_plans
        .retain(|local_coordinate_id, _| active_coordinates.contains(local_coordinate_id));
    world
        .generation_jobs
        .retain(|id| active_coordinates.contains(&id.local_coordinate_id));
    if !physics_interests.matches(&desired_physics) {
        physics_interests.replace(desired_physics);
    }
}

// Publishes only results that still correspond to a pending and desired chunk.
fn commit_generated_chunks(
    voxels: Option<Res<crate::AtomicVoxelRegistry>>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut local_coordinates: Query<(&PcgLocalCoordinate, &mut LocalCoordinate)>,
) {
    let worker = world.generation_worker.clone();
    for _ in 0..MAX_CHUNKS_GENERATED_PER_TICK {
        let Some(result) = worker.try_receive() else {
            break;
        };
        let id = ChunkId {
            local_coordinate_id: result.local_coordinate_id,
            coordinate: result.chunk.coordinate,
        };
        let was_pending = world.generation_jobs.remove(&id);
        let generation_plan = world.generation_plans.get(&id.local_coordinate_id);
        let still_desired =
            generation_plan.is_some_and(|plan| plan.desired.contains(&id.coordinate));
        let loaded_chunks = world.loaded_chunks.get(&id.local_coordinate_id);
        let already_loaded =
            loaded_chunks.is_some_and(|chunks| chunks.contains_key(&id.coordinate));
        if !was_pending || !still_desired || already_loaded {
            continue;
        }
        let Some((_, mut local_coordinate)) = local_coordinates
            .iter_mut()
            .find(|(generated, _)| generated.id == id.local_coordinate_id)
        else {
            continue;
        };
        let builtin;
        let voxels = if let Some(voxels) = voxels.as_deref() {
            voxels
        } else {
            builtin = crate::AtomicVoxelRegistry::builtin();
            &builtin
        };
        replace_generated_chunk(&mut local_coordinate, &result.chunk, voxels);
        world
            .loaded_chunks
            .entry(id.local_coordinate_id)
            .or_default()
            .insert(id.coordinate, result.chunk);
    }
}

/// Queues advertised chunks once, preserving request order.
///
/// Unknown/unadvertised chunks and duplicate pending requests are ignored.
fn enqueue_chunk_requests(
    subscription: &mut PlayerSubscription,
    chunks: impl IntoIterator<Item = ChunkId>,
) {
    for chunk in chunks {
        if !subscription.advertised_chunks.contains_key(&chunk)
            || subscription.pending_requests.contains(&chunk)
        {
            continue;
        }
        subscription.pending_requests.push_back(chunk);
    }
}

// Consumes dirty chunk positions and advances only currently loaded versions.
fn update_authoritative_chunk_versions(
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut local_coordinates: Query<(&PcgLocalCoordinate, &mut LocalCoordinate)>,
) {
    for (generated, mut local_coordinate) in &mut local_coordinates {
        for position in std::mem::take(&mut local_coordinate.changed_chunks) {
            let chunk_id = ChunkId {
                local_coordinate_id: generated.id,
                coordinate: [
                    i64::from(position.x),
                    i64::from(position.y),
                    i64::from(position.z),
                ],
            };
            let is_loaded = local_coordinate.chunks.contains_key(&position);
            if is_loaded {
                world
                    .chunk_versions
                    .entry(chunk_id)
                    .and_modify(|version| *version = version.next())
                    .or_insert(UpdateVersion::INITIAL);
            } else {
                let removed_version = world.chunk_versions.remove(&chunk_id);
                if removed_version.is_none() {
                    log::trace!("removed unloaded Chunk without a tracked version: {chunk_id:?}");
                }
            }
        }
    }
}

/// Returns new or changed versions in canonical coordinate order.
///
/// Chunks absent from `current` are not represented; unload publication is a
/// separate step.
fn changed_chunk_versions(
    advertised: &HashMap<ChunkId, UpdateVersion>,
    current: &HashMap<ChunkId, UpdateVersion>,
) -> Vec<ChunkVersion> {
    let mut chunks = current
        .iter()
        .filter(|(chunk, version)| advertised.get(*chunk) != Some(*version))
        .map(|(chunk, version)| ChunkVersion {
            local_coordinate_id: chunk.local_coordinate_id,
            coordinate: chunk.coordinate,
            version: *version,
        })
        .collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|chunk| {
        (
            chunk.local_coordinate_id.0,
            chunk.coordinate[0],
            chunk.coordinate[1],
            chunk.coordinate[2],
        )
    });
    chunks
}

// Advertises spawn/version/unload deltas, then replaces each connection snapshot.
// A closed bridge is logged but does not retain deltas for retry.
fn publish_subscription_changes(
    pipe: Res<LocalCoordinateServerPipe>,
    virtual_chunks: Res<VirtualChunkIndex>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    generated_coordinates: Query<(&PcgLocalCoordinate, &LocalCoordinate)>,
) {
    let subscription_ids = world.subscriptions.keys().copied().collect::<Vec<_>>();

    for connection_id in subscription_ids {
        let (anchor_id, spawned_coordinates, advertised_chunks) = {
            let subscription = &world.subscriptions[&connection_id];
            (
                subscription.anchor.id,
                subscription.spawned_coordinates.clone(),
                subscription.advertised_chunks.clone(),
            )
        };
        let observation = world.observation_by_anchor.get(&anchor_id).copied();
        let desired_chunks = observation.map_or_else(HashMap::new, |observation| {
            virtual_chunks
                .chunks_in_radius(
                    observation.streaming_center[0],
                    observation.streaming_center[1],
                    observation.streaming_center[2],
                    observation.radius,
                )
                .into_iter()
                .filter_map(|chunk_reference| {
                    streamed_chunk(
                        chunk_reference,
                        &generated_coordinates,
                        &world.loaded_chunks,
                        &world.chunk_versions,
                        observation.scene_id,
                    )
                })
                .collect()
        });
        let desired_coordinates = desired_chunks
            .keys()
            .map(|chunk| chunk.local_coordinate_id)
            .collect::<HashSet<_>>();

        for local_coordinate_id in desired_coordinates.difference(&spawned_coordinates) {
            if pipe
                .0
                .try_send(LocalCoordinateServerEvent::Spawned {
                    connection_id,
                    local_coordinate_id: *local_coordinate_id,
                })
                .is_err()
            {
                log::error!(
                    "cannot publish Local Coordinate spawn: connection_id={}, local_coordinate_id={}, reason=server_bridge_closed",
                    connection_id.0,
                    local_coordinate_id.0
                );
            }
        }

        let mut chunks = changed_chunk_versions(&advertised_chunks, &desired_chunks);
        if let Some(observation) = observation {
            chunks.sort_by(|left, right| {
                nearest_chunk_distance_squared(left.coordinate, &[observation])
                    .total_cmp(&nearest_chunk_distance_squared(
                        right.coordinate,
                        &[observation],
                    ))
                    .then_with(|| {
                        (left.local_coordinate_id.0, left.coordinate)
                            .cmp(&(right.local_coordinate_id.0, right.coordinate))
                    })
            });
        }
        if !chunks.is_empty() {
            let chunk_count = chunks.len();
            if pipe
                .0
                .try_send(LocalCoordinateServerEvent::ChunkVersions {
                    connection_id,
                    chunks,
                })
                .is_err()
            {
                log::error!(
                    "cannot publish Chunk versions: connection_id={}, chunk_count={chunk_count}, reason=server_bridge_closed",
                    connection_id.0
                );
            }
        }
        let mut unloaded = advertised_chunks
            .keys()
            .filter(|chunk| !desired_chunks.contains_key(chunk))
            .copied()
            .collect::<Vec<_>>();
        unloaded.sort_unstable_by_key(|chunk| (chunk.local_coordinate_id.0, chunk.coordinate));
        for unloaded in unloaded {
            if pipe
                .0
                .try_send(LocalCoordinateServerEvent::ChunkUnloaded {
                    connection_id,
                    local_coordinate_id: unloaded.local_coordinate_id,
                    coordinate: unloaded.coordinate,
                })
                .is_err()
            {
                log::error!(
                    "cannot publish Chunk unload: connection_id={}, local_coordinate_id={}, coordinate={:?}, reason=server_bridge_closed",
                    connection_id.0,
                    unloaded.local_coordinate_id.0,
                    unloaded.coordinate
                );
            }
        }

        if let Some(subscription) = world.subscriptions.get_mut(&connection_id) {
            update_subscription_snapshot(subscription, desired_coordinates, desired_chunks);
        }
    }
}

fn update_subscription_snapshot(
    subscription: &mut PlayerSubscription,
    desired_coordinates: HashSet<LocalCoordinateId>,
    desired_chunks: HashMap<ChunkId, UpdateVersion>,
) {
    subscription.spawned_coordinates.extend(desired_coordinates);
    subscription
        .pending_requests
        .retain(|chunk| desired_chunks.contains_key(chunk));
    // Rendering anchors push complete SVO payloads proactively. Version events
    // remain useful for cache hits and backward-compatible request deduplication,
    // but payload delivery does not wait for a client request round trip.
    let mut changed = desired_chunks
        .iter()
        .filter(|(chunk, version)| subscription.advertised_chunks.get(*chunk) != Some(*version))
        .map(|(chunk, _)| *chunk)
        .collect::<Vec<_>>();
    changed.sort_unstable_by_key(|chunk| (chunk.local_coordinate_id.0, chunk.coordinate));
    for chunk in changed {
        if !subscription.pending_requests.contains(&chunk) {
            subscription.pending_requests.push_back(chunk);
        }
    }
    subscription.advertised_chunks = desired_chunks;
}

// Admits bounded per-connection encodes while respecting the global in-flight cap.
fn serve_requested_chunks(
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut generated_coordinates: Query<(&PcgLocalCoordinate, &mut LocalCoordinate)>,
) {
    let connection_ids = world.subscriptions.keys().copied().collect::<Vec<_>>();
    for connection_id in connection_ids {
        for _ in 0..MAX_CHUNK_RESPONSES_PER_TICK_PER_CONNECTION {
            if world.derived_svo_jobs.len() >= MAX_DERIVED_SVO_JOBS_IN_FLIGHT {
                return;
            }
            let Some(requested) = world
                .subscriptions
                .get_mut(&connection_id)
                .and_then(|subscription| subscription.pending_requests.pop_front())
            else {
                break;
            };
            if !world.subscriptions[&connection_id]
                .advertised_chunks
                .contains_key(&requested)
            {
                continue;
            }
            let Some(version) = world.chunk_versions.get(&requested).copied() else {
                continue;
            };
            let Some((_, mut local_coordinate)) = generated_coordinates
                .iter_mut()
                .find(|(generated, _)| generated.id == requested.local_coordinate_id)
            else {
                continue;
            };
            let Some(local_position) =
                crate::local_coordinate::pcg::local_chunk_position(requested.coordinate)
            else {
                continue;
            };
            let Some(chunk) = local_coordinate.chunks.get_mut(&local_position) else {
                continue;
            };
            let job_key = (connection_id, requested);
            if world.derived_svo_jobs.contains_key(&job_key) {
                world
                    .subscriptions
                    .get_mut(&connection_id)
                    .unwrap()
                    .pending_requests
                    .push_back(requested);
                break;
            }
            let chunk_version = ChunkVersion {
                local_coordinate_id: requested.local_coordinate_id,
                coordinate: requested.coordinate,
                version,
            };
            world.derived_svo_jobs.insert(job_key, version);
            if world
                .derived_svo
                .submit(DerivedSvoJob::Encode {
                    connection_id,
                    chunk: chunk_version,
                    source: chunk.svo_source(),
                })
                .is_err()
            {
                let removed_job = world.derived_svo_jobs.remove(&job_key);
                debug_assert!(removed_job.is_some());
                if let Some(subscription) = world.subscriptions.get_mut(&connection_id) {
                    subscription.pending_requests.push_front(requested);
                }
                log::error!(
                    "cannot queue server Chunk encoding: connection_id={}, local_coordinate_id={}, coordinate={:?}, version={}, reason=derived_svo_worker_closed",
                    connection_id.0,
                    requested.local_coordinate_id.0,
                    requested.coordinate,
                    version.value()
                );
                break;
            }
        }
    }
}

// Emits only encoded results whose version remains both current and requested.
fn publish_derived_svo_results(
    pipe: Res<LocalCoordinateServerPipe>,
    mut world: ResMut<LocalCoordinateServerWorld>,
) {
    for _ in 0..MAX_DERIVED_SVO_RESULTS_PER_TICK {
        let Some(result) = world.derived_svo.try_receive() else {
            break;
        };
        match result {
            DerivedSvoResult::Encoded {
                connection_id,
                chunk,
                payload,
            } => {
                if complete_svo_job(&mut world, connection_id, chunk)
                    && pipe
                        .0
                        .try_send(LocalCoordinateServerEvent::ChunkLoaded {
                            connection_id,
                            chunk,
                            payload,
                        })
                        .is_err()
                {
                    log::error!(
                        "cannot publish loaded Chunk: connection_id={}, local_coordinate_id={}, coordinate={:?}, version={}, reason=server_bridge_closed",
                        connection_id.0,
                        chunk.local_coordinate_id.0,
                        chunk.coordinate,
                        chunk.version.value()
                    );
                }
            }
            DerivedSvoResult::Failed {
                connection_id: Some(connection_id),
                chunk,
                error,
            } => {
                let removed_job = world.derived_svo_jobs.remove(&(connection_id, chunk.id()));
                if removed_job.is_none() {
                    log::debug!(
                        "received failure for an inactive server Chunk derivation: connection_id={}, chunk={:?}",
                        connection_id.0,
                        chunk.id()
                    );
                }
                log::warn!(
                    "failed to derive server chunk SVO: connection_id={}, local_coordinate_id={}, coordinate={:?}, error={error}",
                    connection_id.0,
                    chunk.local_coordinate_id.0,
                    chunk.coordinate
                );
            }
            DerivedSvoResult::Decoded { .. }
            | DerivedSvoResult::Failed {
                connection_id: None,
                ..
            } => {}
        }
    }
}

/// Retires one in-flight job and reports whether its exact version is still advertised.
fn complete_svo_job(
    world: &mut LocalCoordinateServerWorld,
    connection_id: ConnectionId,
    chunk: ChunkVersion,
) -> bool {
    let job_key = (connection_id, chunk.id());
    let completed_job = world.derived_svo_jobs.remove(&job_key);
    let was_current_job = completed_job == Some(chunk.version);
    let subscription = world.subscriptions.get(&connection_id);
    let advertised_version =
        subscription.and_then(|subscription| subscription.advertised_chunks.get(&chunk.id()));
    let is_still_requested = advertised_version == Some(&chunk.version);
    was_current_job && is_still_requested
}

/// Converts an absolute observation sphere into conservative coordinate-local bounds.
///
/// Radius is divided by the smallest absolute axis scale so anisotropic scaling
/// cannot exclude visible chunks. Degenerate transforms return `None`.
fn local_observation_region(
    coordinate_transform: &GlobalTransform,
    observation: ObservationRegion,
) -> Option<ObservationRegion> {
    let minimum_scale = coordinate_transform.scale().abs().min_element();
    if minimum_scale <= f32::EPSILON {
        return None;
    }

    let absolute_center = bevy::prelude::Vec3::new(
        observation.center[0] as f32,
        observation.center[1] as f32,
        observation.center[2] as f32,
    );
    let inverse = coordinate_transform.affine().inverse();
    let local_center = inverse.transform_point3(absolute_center);
    let absolute_streaming_center = bevy::prelude::Vec3::new(
        observation.streaming_center[0] as f32,
        observation.streaming_center[1] as f32,
        observation.streaming_center[2] as f32,
    );
    let local_streaming_center = inverse.transform_point3(absolute_streaming_center);
    Some(ObservationRegion {
        center: local_center.as_dvec3().to_array(),
        streaming_center: local_streaming_center.as_dvec3().to_array(),
        radius: observation.radius / f64::from(minimum_scale),
        scene_id: observation.scene_id,
    })
}

/// Resolves a spatial-index reference only if its scene, generated backing, ECS
/// chunk, and authoritative version are all still live.
fn streamed_chunk(
    chunk_reference: crate::ChunkReference,
    generated_coordinates: &Query<(&PcgLocalCoordinate, &LocalCoordinate)>,
    loaded_chunks: &HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
    chunk_versions: &HashMap<ChunkId, UpdateVersion>,
    scene_id: SceneId,
) -> Option<(ChunkId, UpdateVersion)> {
    let entity = chunk_reference.local_coordinate_entity();
    let coordinate_result = generated_coordinates.get(entity);
    let (generated, local_coordinate) = coordinate_result.ok()?;
    if generated.scene_id != scene_id {
        return None;
    }
    let local_position = chunk_reference.local_chunk_position();
    let coordinate = [
        i64::from(local_position.x),
        i64::from(local_position.y),
        i64::from(local_position.z),
    ];
    let generated_chunks = loaded_chunks.get(&generated.id)?;
    if !generated_chunks.contains_key(&coordinate) {
        return None;
    }
    let _loaded_chunk = local_coordinate.chunks.get(&local_position)?;
    let chunk_id = ChunkId {
        local_coordinate_id: generated.id,
        coordinate,
    };
    let version = chunk_versions.get(&chunk_id)?;
    Some((chunk_id, *version))
}

/// Returns squared center distance to the nearest observation, or infinity when empty.
fn nearest_chunk_distance_squared(
    coordinate: ChunkCoordinate,
    observations: &[ObservationRegion],
) -> f64 {
    let edge = CHUNK_EDGE_LENGTH as f64;
    observations
        .iter()
        .map(|observation| {
            (0..3)
                .map(|axis| {
                    let chunk_center = coordinate[axis] as f64 * edge + edge * 0.5;
                    (chunk_center - observation.center[axis]).powi(2)
                })
                .sum::<f64>()
        })
        .reduce(f64::min)
        .unwrap_or(f64::INFINITY)
}

fn absolute_chunk_coordinate(position: [f64; 3]) -> ChunkCoordinate {
    position.map(|axis| (axis / CHUNK_EDGE_LENGTH as f64).floor() as i64)
}

fn distance_to_interval(value: f64, minimum: f64, maximum: f64) -> f64 {
    if value < minimum {
        minimum - value
    } else if value > maximum {
        value - maximum
    } else {
        0.0
    }
}

/// Collects superflat chunks whose closed axis-aligned bounds intersect a sphere.
///
/// The returned set has no iteration-order guarantee.
fn superflat_chunks_intersecting_radius(
    center: [f64; 3],
    radius: f64,
    height: i64,
) -> HashSet<ChunkCoordinate> {
    let edge = CHUNK_EDGE_LENGTH as f64;
    let minimum_x = ((center[0] - radius) / edge).floor() as i64;
    let maximum_x = ((center[0] + radius) / edge).floor() as i64;
    let minimum_z = ((center[2] - radius) / edge).floor() as i64;
    let maximum_z = ((center[2] + radius) / edge).floor() as i64;
    let y = height.div_euclid(CHUNK_EDGE_LENGTH as i64);
    let y_minimum = y as f64 * edge;
    let y_distance = distance_to_interval(center[1], y_minimum, y_minimum + edge);
    if y_distance > radius {
        return HashSet::new();
    }
    let radius_squared = radius * radius;
    let mut chunks = HashSet::new();

    for x in minimum_x..=maximum_x {
        for z in minimum_z..=maximum_z {
            let x_minimum = x as f64 * edge;
            let z_minimum = z as f64 * edge;
            let distance_squared = distance_to_interval(center[0], x_minimum, x_minimum + edge)
                .powi(2)
                + y_distance.powi(2)
                + distance_to_interval(center[2], z_minimum, z_minimum + edge).powi(2);
            if distance_squared <= radius_squared {
                chunks.insert([x, y, z]);
            }
        }
    }

    chunks
}

#[cfg(test)]
mod tests;
