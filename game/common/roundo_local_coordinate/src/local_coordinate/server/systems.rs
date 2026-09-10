use super::*;

impl Plugin for LocalCoordinateServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LocalCoordinateBasePlugin, LocalCoordinatePhysicsPlugin))
            .insert_resource(GeneratedLocalCoordinates(
                self.generated_coordinates.clone(),
            ))
            .init_resource::<LocalCoordinateServerWorld>()
            .init_resource::<LocalCoordinateObservationInput>()
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

pub(super) struct PlayerSubscription {
    player_id: PlayerId,
    view_distance_chunks: u16,
    spawned_coordinates: HashSet<LocalCoordinateId>,
    advertised_chunks: HashMap<ChunkId, UpdateVersion>,
    pending_requests: VecDeque<ChunkId>,
}

#[derive(Clone, Copy)]
pub(super) struct ObservationRegion {
    center: [f64; 3],
    radius: f64,
    scene_id: SceneId,
}

#[derive(Default)]
pub(super) struct GenerationPlan {
    signature: Vec<([i64; 3], u16)>,
    desired: HashSet<ChunkCoordinate>,
    pending: VecDeque<ChunkCoordinate>,
    evicting: VecDeque<ChunkCoordinate>,
}

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

fn prepare_player_chunks(
    pipe: Res<LocalCoordinateServerPipe>,
    observation_input: Res<LocalCoordinateObservationInput>,
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
                let view_distance_chunks = world
                    .requested_view_distances
                    .get(&connection_id)
                    .copied()
                    .unwrap_or(DEFAULT_CHUNK_VIEW_DISTANCE);
                world.subscriptions.insert(
                    connection_id,
                    PlayerSubscription {
                        player_id,
                        view_distance_chunks,
                        spawned_coordinates: HashSet::new(),
                        advertised_chunks: HashMap::new(),
                        pending_requests: VecDeque::new(),
                    },
                );
            }
            LocalCoordinateServerCommand::UnsubscribePlayer { connection_id } => {
                world.subscriptions.remove(&connection_id);
                world.requested_view_distances.remove(&connection_id);
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
                    subscription.view_distance_chunks = chunks;
                }
            }
        }
    }

    let subscribed_players = world.subscriptions.values().fold(
        HashMap::<PlayerId, u16>::new(),
        |mut players, subscription| {
            players
                .entry(subscription.player_id)
                .and_modify(|distance| {
                    *distance = (*distance).max(subscription.view_distance_chunks)
                })
                .or_insert(subscription.view_distance_chunks);
            players
        },
    );
    world.observation_by_player.clear();
    for observer in observation_input.observers() {
        if let Some(&view_distance_chunks) = subscribed_players.get(&observer.player_id) {
            world.observation_by_player.insert(
                observer.player_id,
                ObservationRegion {
                    center: observer.position,
                    radius: f64::from(view_distance_chunks) * CHUNK_EDGE_LENGTH as f64,
                    scene_id: observer.scene_id,
                },
            );
        }
    }

    let mut active_coordinates = HashSet::new();
    let mut desired_physics = HashMap::new();
    let mut generation_budget = MAX_CHUNKS_GENERATED_PER_TICK
        .min(MAX_CHUNK_GENERATION_JOBS_IN_FLIGHT.saturating_sub(world.generation_jobs.len()));
    let generation_worker = world.generation_worker.clone();

    for (generated, coordinate_transform, mut local_coordinate) in &mut local_coordinates {
        active_coordinates.insert(generated.id);
        let Some(scene_extent) = observation_input.scene_extent(generated.scene_id) else {
            continue;
        };
        let observations = world
            .observation_by_player
            .values()
            .filter(|observation| observation.scene_id == generated.scene_id)
            .filter_map(|observation| local_observation_region(coordinate_transform, *observation))
            .collect::<Vec<_>>();
        desired_physics.insert(
            generated.id,
            observations
                .iter()
                .flat_map(|observation| {
                    superflat_chunks_intersecting_torus_radius(
                        observation.center,
                        PHYSICS_CHUNK_RADIUS * CHUNK_EDGE_LENGTH as f64,
                        generated.generator.height,
                        scene_extent,
                    )
                })
                .filter(|coordinate| chunk_intersects_space(*coordinate, scene_extent))
                .filter_map(crate::local_coordinate::pcg::local_chunk_position)
                .collect(),
        );
        let mut signature = observations
            .iter()
            .map(|observation| {
                (
                    observation
                        .center
                        .map(|axis| (axis / CHUNK_EDGE_LENGTH as f64).floor() as i64),
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
                    center: center.map(|axis| (axis as f64 + 0.5) * CHUNK_EDGE_LENGTH as f64),
                    radius: f64::from(*distance) * CHUNK_EDGE_LENGTH as f64,
                    scene_id: generated.scene_id,
                })
                .collect::<Vec<_>>();
            let desired_chunks = snapped_observations
                .iter()
                .flat_map(|observation| {
                    superflat_chunks_intersecting_torus_radius(
                        observation.center,
                        observation.radius,
                        generated.generator.height,
                        scene_extent,
                    )
                })
                .filter(|coordinate| chunk_intersects_space(*coordinate, scene_extent))
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
                nearest_chunk_distance_squared(*left, &snapped_observations, scene_extent)
                    .total_cmp(&nearest_chunk_distance_squared(
                        *right,
                        &snapped_observations,
                        scene_extent,
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
            loaded_chunks.remove(&coordinate);
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
            generation_worker.submit(ChunkGenerationJob {
                local_coordinate_id: generated.id,
                coordinate,
                generator: generated.generator,
            });
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

fn commit_generated_chunks(
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
        if !world.generation_jobs.remove(&id)
            || !world
                .generation_plans
                .get(&id.local_coordinate_id)
                .is_some_and(|plan| plan.desired.contains(&id.coordinate))
            || world
                .loaded_chunks
                .get(&id.local_coordinate_id)
                .is_some_and(|chunks| chunks.contains_key(&id.coordinate))
        {
            continue;
        }
        let Some((_, mut local_coordinate)) = local_coordinates
            .iter_mut()
            .find(|(generated, _)| generated.id == id.local_coordinate_id)
        else {
            continue;
        };
        replace_generated_chunk(&mut local_coordinate, &result.chunk);
        world
            .loaded_chunks
            .entry(id.local_coordinate_id)
            .or_default()
            .insert(id.coordinate, result.chunk);
    }
}

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
                world.chunk_versions.remove(&chunk_id);
            }
        }
    }
}

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

fn publish_subscription_changes(
    pipe: Res<LocalCoordinateServerPipe>,
    virtual_chunks: Res<VirtualChunkIndex>,
    observation_input: Res<LocalCoordinateObservationInput>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    generated_coordinates: Query<(&PcgLocalCoordinate, &LocalCoordinate)>,
) {
    let subscription_ids = world.subscriptions.keys().copied().collect::<Vec<_>>();

    for connection_id in subscription_ids {
        let (player_id, spawned_coordinates, advertised_chunks) = {
            let subscription = &world.subscriptions[&connection_id];
            (
                subscription.player_id,
                subscription.spawned_coordinates.clone(),
                subscription.advertised_chunks.clone(),
            )
        };
        let observation = world.observation_by_player.get(&player_id).copied();
        let desired_chunks = observation.map_or_else(HashMap::new, |observation| {
            let Some(scene_extent) = observation_input.scene_extent(observation.scene_id) else {
                return HashMap::new();
            };
            virtual_chunks
                .chunks_in_torus_radius(observation.center, observation.radius, scene_extent)
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
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::Spawned {
                connection_id,
                local_coordinate_id: *local_coordinate_id,
            });
        }

        let mut chunks = changed_chunk_versions(&advertised_chunks, &desired_chunks);
        if let Some(observation) = observation {
            let scene_extent = observation_input
                .scene_extent(observation.scene_id)
                .expect("an observed scene has a validated extent");
            chunks.sort_by(|left, right| {
                nearest_chunk_distance_squared(left.coordinate, &[observation], scene_extent)
                    .total_cmp(&nearest_chunk_distance_squared(
                        right.coordinate,
                        &[observation],
                        scene_extent,
                    ))
                    .then_with(|| {
                        (left.local_coordinate_id.0, left.coordinate)
                            .cmp(&(right.local_coordinate_id.0, right.coordinate))
                    })
            });
        }
        if !chunks.is_empty() {
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkVersions {
                connection_id,
                chunks,
            });
        }
        let mut unloaded = advertised_chunks
            .keys()
            .filter(|chunk| !desired_chunks.contains_key(chunk))
            .copied()
            .collect::<Vec<_>>();
        unloaded.sort_unstable_by_key(|chunk| (chunk.local_coordinate_id.0, chunk.coordinate));
        for unloaded in unloaded {
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkUnloaded {
                connection_id,
                local_coordinate_id: unloaded.local_coordinate_id,
                coordinate: unloaded.coordinate,
            });
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
    subscription.advertised_chunks = desired_chunks;
}

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
            world.derived_svo.submit(DerivedSvoJob::Encode {
                connection_id,
                chunk: chunk_version,
                source: chunk.svo_source(),
            });
        }
    }
}

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
                if complete_svo_job(&mut world, connection_id, chunk) {
                    let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkLoaded {
                        connection_id,
                        chunk,
                        payload,
                    });
                }
            }
            DerivedSvoResult::Failed {
                connection_id: Some(connection_id),
                chunk,
                error,
            } => {
                world.derived_svo_jobs.remove(&(connection_id, chunk.id()));
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

fn complete_svo_job(
    world: &mut LocalCoordinateServerWorld,
    connection_id: ConnectionId,
    chunk: ChunkVersion,
) -> bool {
    let job_key = (connection_id, chunk.id());
    let was_current_job = world.derived_svo_jobs.remove(&job_key) == Some(chunk.version);
    let is_still_requested = world
        .subscriptions
        .get(&connection_id)
        .and_then(|subscription| subscription.advertised_chunks.get(&chunk.id()))
        == Some(&chunk.version);
    was_current_job && is_still_requested
}

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
    let local_center = coordinate_transform
        .affine()
        .inverse()
        .transform_point3(absolute_center);
    Some(ObservationRegion {
        center: local_center.as_dvec3().to_array(),
        radius: observation.radius / f64::from(minimum_scale),
        scene_id: observation.scene_id,
    })
}

fn streamed_chunk(
    chunk_reference: crate::ChunkReference,
    generated_coordinates: &Query<(&PcgLocalCoordinate, &LocalCoordinate)>,
    loaded_chunks: &HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
    chunk_versions: &HashMap<ChunkId, UpdateVersion>,
    scene_id: SceneId,
) -> Option<(ChunkId, UpdateVersion)> {
    let (generated, local_coordinate) = generated_coordinates
        .get(chunk_reference.local_coordinate_entity())
        .ok()?;
    if generated.scene_id != scene_id {
        return None;
    }
    let local_position = chunk_reference.local_chunk_position();
    let coordinate = [
        i64::from(local_position.x),
        i64::from(local_position.y),
        i64::from(local_position.z),
    ];
    if !loaded_chunks.get(&generated.id)?.contains_key(&coordinate) {
        return None;
    }
    local_coordinate.chunks.get(&local_position)?;
    let chunk_id = ChunkId {
        local_coordinate_id: generated.id,
        coordinate,
    };
    Some((chunk_id, *chunk_versions.get(&chunk_id)?))
}

fn nearest_chunk_distance_squared(
    coordinate: ChunkCoordinate,
    observations: &[ObservationRegion],
    scene_extent: [f32; 3],
) -> f64 {
    let edge = CHUNK_EDGE_LENGTH as f64;
    observations
        .iter()
        .map(|observation| {
            (0..3)
                .map(|axis| {
                    let chunk_center = coordinate[axis] as f64 * edge + edge * 0.5;
                    periodic_delta(
                        chunk_center,
                        observation.center[axis],
                        f64::from(scene_extent[axis]),
                    )
                    .powi(2)
                })
                .sum::<f64>()
        })
        .reduce(f64::min)
        .unwrap_or(f64::INFINITY)
}

fn periodic_delta(first: f64, second: f64, extent: f64) -> f64 {
    let difference = (first - second).abs().rem_euclid(extent);
    difference.min(extent - difference)
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

fn periodic_distance_to_interval(value: f64, minimum: f64, maximum: f64, extent: f64) -> f64 {
    [-extent, 0.0, extent]
        .into_iter()
        .map(|offset| distance_to_interval(value, minimum + offset, maximum + offset))
        .reduce(f64::min)
        .unwrap_or(f64::INFINITY)
}

fn superflat_chunks_intersecting_torus_radius(
    center: [f64; 3],
    radius: f64,
    height: i64,
    scene_extent: [f32; 3],
) -> HashSet<ChunkCoordinate> {
    let edge = CHUNK_EDGE_LENGTH as f64;
    let chunk_counts = scene_extent.map(|extent| (f64::from(extent) / edge).floor() as i64);
    if chunk_counts.into_iter().any(|count| count <= 0) {
        return HashSet::new();
    }
    let minimum_x = ((center[0] - radius) / edge).floor() as i64;
    let maximum_x = ((center[0] + radius) / edge).floor() as i64;
    let minimum_z = ((center[2] - radius) / edge).floor() as i64;
    let maximum_z = ((center[2] + radius) / edge).floor() as i64;
    let y = height.div_euclid(CHUNK_EDGE_LENGTH as i64);
    let canonical_y = y.rem_euclid(chunk_counts[1]);
    let y_minimum = canonical_y as f64 * edge;
    let y_distance = periodic_distance_to_interval(
        center[1],
        y_minimum,
        y_minimum + edge,
        f64::from(scene_extent[1]),
    );
    if y_distance > radius {
        return HashSet::new();
    }
    let radius_squared = radius * radius;
    let mut chunks = HashSet::new();

    for image_x in minimum_x..=maximum_x {
        for image_z in minimum_z..=maximum_z {
            let x_minimum = image_x as f64 * edge;
            let z_minimum = image_z as f64 * edge;
            let distance_squared = distance_to_interval(center[0], x_minimum, x_minimum + edge)
                .powi(2)
                + y_distance.powi(2)
                + distance_to_interval(center[2], z_minimum, z_minimum + edge).powi(2);
            if distance_squared <= radius_squared {
                chunks.insert([
                    image_x.rem_euclid(chunk_counts[0]),
                    canonical_y,
                    image_z.rem_euclid(chunk_counts[2]),
                ]);
            }
        }
    }

    chunks
}

fn chunk_intersects_space(coordinate: ChunkCoordinate, scene_extent: [f32; 3]) -> bool {
    let edge = CHUNK_EDGE_LENGTH as f64;
    coordinate
        .iter()
        .zip(scene_extent)
        .all(|(coordinate, extent)| {
            let minimum = *coordinate as f64 * edge;
            let maximum = minimum + edge;
            maximum > 0.0 && minimum < f64::from(extent)
        })
}

#[cfg(test)]
mod tests;
