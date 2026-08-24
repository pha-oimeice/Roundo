use super::*;

impl Plugin for LocalCoordinateServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LocalCoordinateBasePlugin, LocalCoordinatePhysicsPlugin))
            .insert_resource(GeneratedLocalCoordinates(
                self.generated_coordinates.clone(),
            ))
            .init_resource::<LocalCoordinateServerWorld>()
            .insert_resource(LocalCoordinateServerPipe(self.pipe.endpoint_b()))
            .add_systems(Startup, spawn_generated_local_coordinates)
            .add_systems(
                FixedUpdate,
                prepare_player_chunks
                    .after(LocalCoordinateSet::ApplyCrud)
                    .after(PresenceServerSet::SceneConstraints)
                    .before(rebuild_virtual_chunk_index),
            )
            .add_systems(
                FixedUpdate,
                update_authoritative_chunk_versions
                    .after(prepare_player_chunks)
                    .before(rebuild_virtual_chunk_index),
            )
            .add_systems(
                FixedUpdate,
                publish_subscription_changes.after(rebuild_virtual_chunk_index),
            )
            .add_systems(
                FixedUpdate,
                serve_requested_chunks.after(publish_subscription_changes),
            )
            .add_systems(
                FixedUpdate,
                publish_derived_svo_results.after(serve_requested_chunks),
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

fn spawn_generated_local_coordinates(
    mut commands: Commands,
    coordinates: Res<GeneratedLocalCoordinates>,
) {
    for coordinate in &coordinates.0 {
        commands.spawn((
            LocalCoordinate::default(),
            *coordinate,
            RigidBody::Static,
            LocalCoordinateTransform::default(),
        ));
    }
}

fn prepare_player_chunks(
    pipe: Res<LocalCoordinateServerPipe>,
    scenes: Res<ServerSceneWorlds>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    players: Query<(&Player, &PlayerScene, &GlobalTransform), With<ServerPlayer>>,
    mut local_coordinates: Query<(&PcgLocalCoordinate, &GlobalTransform, &mut LocalCoordinate)>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            LocalCoordinateServerCommand::SubscribePlayer {
                connection_id,
                player_id,
            } => {
                world.subscriptions.insert(
                    connection_id,
                    PlayerSubscription {
                        player_id,
                        spawned_coordinates: HashSet::new(),
                        advertised_chunks: HashMap::new(),
                        pending_requests: VecDeque::new(),
                    },
                );
            }
            LocalCoordinateServerCommand::UnsubscribePlayer { connection_id } => {
                world.subscriptions.remove(&connection_id);
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
        }
    }

    let subscribed_players = world
        .subscriptions
        .values()
        .map(|subscription| subscription.player_id)
        .collect::<HashSet<_>>();
    world.observation_by_player.clear();
    for (player, scene, transform) in &players {
        if subscribed_players.contains(&player.id) {
            world.observation_by_player.insert(
                player.id,
                ObservationRegion {
                    center: transform.translation().as_dvec3().to_array(),
                    radius: PLAYER_CHUNK_LOAD_RADIUS,
                    scene_id: scene.scene_id,
                },
            );
        }
    }

    let mut active_coordinates = HashSet::new();
    let mut generation_budget = MAX_CHUNKS_GENERATED_PER_TICK;

    for (generated, coordinate_transform, mut local_coordinate) in &mut local_coordinates {
        active_coordinates.insert(generated.id);
        let Some(space) = scenes.space(generated.scene_id) else {
            continue;
        };
        let desired_chunks = world
            .observation_by_player
            .values()
            .filter(|observation| observation.scene_id == generated.scene_id)
            .filter_map(|observation| local_observation_region(coordinate_transform, *observation))
            .flat_map(|observation| {
                chunks_intersecting_radius(observation.center, observation.radius)
            })
            .filter(|coordinate| chunk_intersects_space(*coordinate, space))
            .collect::<HashSet<_>>();
        let loaded_chunks = world.loaded_chunks.entry(generated.id).or_default();
        let mut missing = desired_chunks
            .iter()
            .filter(|coordinate| !loaded_chunks.contains_key(*coordinate))
            .copied()
            .collect::<Vec<_>>();
        missing.sort_unstable();
        for coordinate in missing.into_iter().take(generation_budget) {
            let chunk =
                generated
                    .generator
                    .generate_chunk(coordinate[0], coordinate[1], coordinate[2]);
            replace_generated_chunk(&mut local_coordinate, &chunk);
            loaded_chunks.insert(coordinate, chunk);
            generation_budget = generation_budget.saturating_sub(1);
        }
    }

    world
        .loaded_chunks
        .retain(|local_coordinate_id, _| active_coordinates.contains(local_coordinate_id));
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
            world
                .chunk_versions
                .entry(chunk_id)
                .and_modify(|version| *version = version.next())
                .or_insert(UpdateVersion::INITIAL);
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
            virtual_chunks
                .chunks_in_radius(
                    observation.center[0],
                    observation.center[1],
                    observation.center[2],
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
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::Spawned {
                connection_id,
                local_coordinate_id: *local_coordinate_id,
            });
        }

        let chunks = changed_chunk_versions(&advertised_chunks, &desired_chunks);
        if !chunks.is_empty() {
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkVersions {
                connection_id,
                chunks,
            });
        }

        if let Some(subscription) = world.subscriptions.get_mut(&connection_id) {
            retain_subscription_snapshot(subscription, desired_coordinates, desired_chunks);
        }
    }
}

fn retain_subscription_snapshot(
    subscription: &mut PlayerSubscription,
    desired_coordinates: HashSet<LocalCoordinateId>,
    desired_chunks: HashMap<ChunkId, UpdateVersion>,
) {
    subscription.spawned_coordinates.extend(desired_coordinates);
    subscription.advertised_chunks.extend(desired_chunks);
}

fn serve_requested_chunks(
    mut world: ResMut<LocalCoordinateServerWorld>,
    mut generated_coordinates: Query<(&PcgLocalCoordinate, &mut LocalCoordinate)>,
) {
    let connection_ids = world.subscriptions.keys().copied().collect::<Vec<_>>();
    for connection_id in connection_ids {
        for _ in 0..MAX_CHUNK_RESPONSES_PER_TICK_PER_CONNECTION {
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
            let Some(local_position) = local_chunk_position(requested.coordinate) else {
                continue;
            };
            let Some(chunk) = local_coordinate.chunks.get_mut(&local_position) else {
                continue;
            };
            let chunk_version = ChunkVersion {
                local_coordinate_id: requested.local_coordinate_id,
                coordinate: requested.coordinate,
                version,
            };
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
    world: Res<LocalCoordinateServerWorld>,
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
                let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkLoaded {
                    connection_id,
                    chunk,
                    payload,
                });
            }
            DerivedSvoResult::Failed {
                connection_id: Some(connection_id),
                chunk,
                error,
            } => log::warn!(
                "failed to derive server chunk SVO: connection_id={}, local_coordinate_id={}, coordinate={:?}, error={error}",
                connection_id.0,
                chunk.local_coordinate_id.0,
                chunk.coordinate
            ),
            DerivedSvoResult::Decoded { .. }
            | DerivedSvoResult::Failed {
                connection_id: None,
                ..
            } => {}
        }
    }
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

fn local_chunk_position(coordinate: ChunkCoordinate) -> Option<bevy::prelude::IVec3> {
    let edge_length = CHUNK_EDGE_LENGTH as i32;
    let minimum = i64::from(i32::MIN / edge_length);
    let maximum = i64::from(i32::MAX / edge_length);
    if coordinate
        .iter()
        .any(|value| !(minimum..=maximum).contains(value))
    {
        return None;
    }
    Some(bevy::prelude::IVec3::new(
        coordinate[0] as i32,
        coordinate[1] as i32,
        coordinate[2] as i32,
    ))
}

fn chunks_intersecting_radius(center: [f64; 3], radius: f64) -> HashSet<ChunkCoordinate> {
    let edge = CHUNK_EDGE_LENGTH as f64;
    let minimum = center.map(|value| ((value - radius) / edge).floor() as i64);
    let maximum = center.map(|value| ((value + radius) / edge).floor() as i64);
    let radius_squared = radius * radius;
    let mut chunks = HashSet::new();

    for x in minimum[0]..=maximum[0] {
        for y in minimum[1]..=maximum[1] {
            for z in minimum[2]..=maximum[2] {
                let coordinate = [x, y, z];
                let distance_squared = (0..3)
                    .map(|axis| {
                        let chunk_min = coordinate[axis] as f64 * edge;
                        let chunk_max = chunk_min + edge;
                        if center[axis] < chunk_min {
                            (chunk_min - center[axis]).powi(2)
                        } else if center[axis] > chunk_max {
                            (center[axis] - chunk_max).powi(2)
                        } else {
                            0.0
                        }
                    })
                    .sum::<f64>();
                if distance_squared <= radius_squared {
                    chunks.insert(coordinate);
                }
            }
        }
    }

    chunks
}

fn chunk_intersects_space(coordinate: ChunkCoordinate, space: TorusSpace) -> bool {
    let edge = CHUNK_EDGE_LENGTH as f64;
    coordinate
        .iter()
        .zip(space.size())
        .all(|(coordinate, extent)| {
            let minimum = *coordinate as f64 * edge;
            let maximum = minimum + edge;
            maximum > 0.0 && minimum < f64::from(extent)
        })
}

#[cfg(test)]
mod tests;
