use crate::local_coordinate::{
    base::LocalCoordinateBasePlugin,
    data::LocalCoordinate,
    pcg::replace_generated_chunk,
    physics::LocalCoordinatePhysicsPlugin,
    virtual_chunk::{ChunkReference, VirtualChunkIndex, rebuild_virtual_chunk_index},
};
use crate::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, GeneratedChunk, InfiniteSphereGenerator, LocalCoordinateId,
};
use avian3d::prelude::RigidBody;
use bevy::prelude::{
    App, Commands, Component, FixedUpdate, GlobalTransform, IntoScheduleConfigs, Plugin, Query,
    Res, ResMut, Resource, Startup, Transform, With,
};
use roundo_networking::ConnectionId;
use roundo_presence::{Player, PlayerId, ServerPlayer};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet};

pub const DEFAULT_PCG_LOCAL_COORDINATE_ID: LocalCoordinateId = LocalCoordinateId(1);
pub const PLAYER_CHUNK_LOAD_RADIUS: f64 = 64.0;
const MAX_CHUNKS_GENERATED_PER_TICK: usize = 16;

pub type LocalCoordinateServerIpc =
    CrossbeamThreadPipeEndpointA<LocalCoordinateServerCommand, LocalCoordinateServerEvent>;

#[derive(Clone)]
pub struct LocalCoordinateServerPlugin {
    pipe: CrossbeamThreadPipe<LocalCoordinateServerCommand, LocalCoordinateServerEvent>,
    generated_coordinates: Vec<PcgLocalCoordinate>,
}

impl LocalCoordinateServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                InfiniteSphereGenerator::default(),
            )],
        }
    }

    pub fn with_generator(generator: InfiniteSphereGenerator) -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generated_coordinates: vec![PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                generator,
            )],
        }
    }

    pub fn with_generated_coordinate(
        mut self,
        local_coordinate_id: LocalCoordinateId,
        generator: InfiniteSphereGenerator,
    ) -> Self {
        if let Some(existing) = self
            .generated_coordinates
            .iter_mut()
            .find(|coordinate| coordinate.id == local_coordinate_id)
        {
            existing.generator = generator;
        } else {
            self.generated_coordinates
                .push(PcgLocalCoordinate::new(local_coordinate_id, generator));
        }
        self
    }

    pub fn ipc(&self) -> LocalCoordinateServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for LocalCoordinateServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

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
                prepare_player_chunks.before(rebuild_virtual_chunk_index),
            )
            .add_systems(
                FixedUpdate,
                publish_subscription_changes.after(rebuild_virtual_chunk_index),
            );
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct PcgLocalCoordinate {
    pub id: LocalCoordinateId,
    pub generator: InfiniteSphereGenerator,
}

impl PcgLocalCoordinate {
    pub const fn new(id: LocalCoordinateId, generator: InfiniteSphereGenerator) -> Self {
        Self { id, generator }
    }
}

#[derive(Clone, Debug)]
pub enum LocalCoordinateServerCommand {
    SubscribePlayer {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    UnsubscribePlayer {
        connection_id: ConnectionId,
    },
}

#[derive(Clone, Debug)]
pub enum LocalCoordinateServerEvent {
    Spawned {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
    },
    Despawned {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
    },
    ChunkLoaded {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
        chunk: GeneratedChunk,
    },
    ChunkUnloaded {
        connection_id: ConnectionId,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    },
}

#[derive(Resource, Default)]
pub struct LocalCoordinateServerWorld {
    loaded_chunks: HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
    observation_by_player: HashMap<PlayerId, ObservationRegion>,
    subscriptions: HashMap<ConnectionId, PlayerSubscription>,
}

impl LocalCoordinateServerWorld {
    pub fn chunk(
        &self,
        local_coordinate_id: LocalCoordinateId,
        coordinate: ChunkCoordinate,
    ) -> Option<&GeneratedChunk> {
        self.loaded_chunks
            .get(&local_coordinate_id)?
            .get(&coordinate)
    }

    pub fn loaded_chunk_count(&self, local_coordinate_id: LocalCoordinateId) -> usize {
        self.loaded_chunks
            .get(&local_coordinate_id)
            .map_or(0, HashMap::len)
    }

    pub fn coordinate_count(&self) -> usize {
        self.loaded_chunks.len()
    }
}

#[derive(Resource, Clone)]
struct GeneratedLocalCoordinates(Vec<PcgLocalCoordinate>);

#[derive(Resource, Clone)]
struct LocalCoordinateServerPipe(
    CrossbeamThreadPipeEndpointB<LocalCoordinateServerCommand, LocalCoordinateServerEvent>,
);

struct PlayerSubscription {
    player_id: PlayerId,
    spawned_coordinates: HashSet<LocalCoordinateId>,
    sent_chunks: HashMap<ChunkReference, StreamedChunk>,
}

#[derive(Clone, Copy)]
struct ObservationRegion {
    center: [f64; 3],
    radius: f64,
}

#[derive(Clone, Copy)]
struct StreamedChunk {
    local_coordinate_id: LocalCoordinateId,
    coordinate: ChunkCoordinate,
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
            Transform::default(),
        ));
    }
}

fn prepare_player_chunks(
    pipe: Res<LocalCoordinateServerPipe>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    players: Query<(&Player, &GlobalTransform), With<ServerPlayer>>,
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
                        sent_chunks: HashMap::new(),
                    },
                );
            }
            LocalCoordinateServerCommand::UnsubscribePlayer { connection_id } => {
                world.subscriptions.remove(&connection_id);
            }
        }
    }

    let subscribed_players = world
        .subscriptions
        .values()
        .map(|subscription| subscription.player_id)
        .collect::<HashSet<_>>();
    world.observation_by_player.clear();
    for (player, transform) in &players {
        if subscribed_players.contains(&player.id) {
            world.observation_by_player.insert(
                player.id,
                ObservationRegion {
                    center: transform.translation().as_dvec3().to_array(),
                    radius: PLAYER_CHUNK_LOAD_RADIUS,
                },
            );
        }
    }

    let mut active_coordinates = HashSet::new();
    let mut generation_budget = MAX_CHUNKS_GENERATED_PER_TICK;

    for (generated, coordinate_transform, mut local_coordinate) in &mut local_coordinates {
        active_coordinates.insert(generated.id);
        let desired_chunks = world
            .observation_by_player
            .values()
            .filter_map(|observation| local_observation_region(coordinate_transform, *observation))
            .flat_map(|observation| {
                chunks_intersecting_radius(observation.center, observation.radius)
            })
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

fn publish_subscription_changes(
    pipe: Res<LocalCoordinateServerPipe>,
    virtual_chunks: Res<VirtualChunkIndex>,
    mut world: ResMut<LocalCoordinateServerWorld>,
    generated_coordinates: Query<&PcgLocalCoordinate>,
) {
    let subscription_ids = world.subscriptions.keys().copied().collect::<Vec<_>>();

    for connection_id in subscription_ids {
        let (player_id, spawned_coordinates, sent_chunks) = {
            let subscription = &world.subscriptions[&connection_id];
            (
                subscription.player_id,
                subscription.spawned_coordinates.clone(),
                subscription.sent_chunks.clone(),
            )
        };
        let desired_chunks =
            world
                .observation_by_player
                .get(&player_id)
                .map_or_else(HashMap::new, |observation| {
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
                            )
                            .map(|chunk| (chunk_reference, chunk))
                        })
                        .collect()
                });
        let desired_coordinates = desired_chunks
            .values()
            .map(|chunk| chunk.local_coordinate_id)
            .collect::<HashSet<_>>();

        for local_coordinate_id in desired_coordinates.difference(&spawned_coordinates) {
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::Spawned {
                connection_id,
                local_coordinate_id: *local_coordinate_id,
            });
        }

        for (chunk_reference, chunk) in &desired_chunks {
            if !sent_chunks.contains_key(chunk_reference)
                && let Some(generated_chunk) = world
                    .loaded_chunks
                    .get(&chunk.local_coordinate_id)
                    .and_then(|chunks| chunks.get(&chunk.coordinate))
            {
                let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkLoaded {
                    connection_id,
                    local_coordinate_id: chunk.local_coordinate_id,
                    chunk: generated_chunk.clone(),
                });
            }
        }
        for (chunk_reference, chunk) in &sent_chunks {
            if !desired_chunks.contains_key(chunk_reference)
                && desired_coordinates.contains(&chunk.local_coordinate_id)
            {
                let _ = pipe.0.try_send(LocalCoordinateServerEvent::ChunkUnloaded {
                    connection_id,
                    local_coordinate_id: chunk.local_coordinate_id,
                    coordinate: chunk.coordinate,
                });
            }
        }
        for local_coordinate_id in spawned_coordinates.difference(&desired_coordinates) {
            let _ = pipe.0.try_send(LocalCoordinateServerEvent::Despawned {
                connection_id,
                local_coordinate_id: *local_coordinate_id,
            });
        }

        if let Some(subscription) = world.subscriptions.get_mut(&connection_id) {
            subscription.spawned_coordinates = desired_coordinates;
            subscription.sent_chunks = desired_chunks;
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
    })
}

fn streamed_chunk(
    chunk_reference: ChunkReference,
    generated_coordinates: &Query<&PcgLocalCoordinate>,
    loaded_chunks: &HashMap<LocalCoordinateId, HashMap<ChunkCoordinate, GeneratedChunk>>,
) -> Option<StreamedChunk> {
    let generated = generated_coordinates
        .get(chunk_reference.local_coordinate_entity())
        .ok()?;
    let local_position = chunk_reference.local_chunk_position();
    let coordinate = [
        i64::from(local_position.x),
        i64::from(local_position.y),
        i64::from(local_position.z),
    ];
    loaded_chunks
        .get(&generated.id)?
        .contains_key(&coordinate)
        .then_some(StreamedChunk {
            local_coordinate_id: generated.id,
            coordinate,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{Schedule, Vec3, World};

    #[test]
    fn generated_coordinates_are_static_identity_local_coordinates() {
        let mut world = World::new();
        world.insert_resource(GeneratedLocalCoordinates(vec![
            PcgLocalCoordinate::new(
                DEFAULT_PCG_LOCAL_COORDINATE_ID,
                InfiniteSphereGenerator::new(1),
            ),
            PcgLocalCoordinate::new(LocalCoordinateId(2), InfiniteSphereGenerator::new(2)),
        ]));
        let mut schedule = Schedule::default();
        schedule.add_systems(spawn_generated_local_coordinates);
        schedule.run(&mut world);

        let mut query = world.query::<(
            &PcgLocalCoordinate,
            &LocalCoordinate,
            &RigidBody,
            &Transform,
        )>();
        let coordinates = query.iter(&world).collect::<Vec<_>>();
        assert_eq!(coordinates.len(), 2);
        for (_, _, rigid_body, transform) in coordinates {
            assert_eq!(*rigid_body, RigidBody::Static);
            assert_eq!(transform.translation, Vec3::ZERO);
            assert_eq!(transform.rotation, Default::default());
            assert_eq!(transform.scale, Vec3::ONE);
        }
    }
}
