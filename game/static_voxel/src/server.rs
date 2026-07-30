use crate::{CHUNK_EDGE_LENGTH, ChunkCoordinate, GeneratedChunk, InfiniteSphereGenerator};
use bevy::prelude::{App, FixedUpdate, Plugin, Query, Res, ResMut, Resource, Transform, With};
use roundo_character::{Character, CharacterId, ServerCharacter};
use roundo_networking::ConnectionId;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet};

pub const CHARACTER_CHUNK_LOAD_RADIUS: f64 = 64.0;
const MAX_CHUNKS_GENERATED_PER_TICK: usize = 16;

pub type StaticVoxelServerIpc =
    CrossbeamThreadPipeEndpointA<StaticVoxelServerCommand, StaticVoxelServerEvent>;

#[derive(Clone)]
pub struct StaticVoxelServerPlugin {
    pipe: CrossbeamThreadPipe<StaticVoxelServerCommand, StaticVoxelServerEvent>,
    generator: InfiniteSphereGenerator,
}

impl StaticVoxelServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generator: InfiniteSphereGenerator::default(),
        }
    }

    pub fn with_generator(generator: InfiniteSphereGenerator) -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
            generator,
        }
    }

    pub fn ipc(&self) -> StaticVoxelServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for StaticVoxelServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for StaticVoxelServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(StaticVoxelServerWorld::new(self.generator))
            .insert_resource(StaticVoxelServerPipe(self.pipe.endpoint_b()))
            .add_systems(FixedUpdate, maintain_character_chunks);
    }
}

#[derive(Clone, Debug)]
pub enum StaticVoxelServerCommand {
    SubscribeCharacter {
        connection_id: ConnectionId,
        character_id: CharacterId,
    },
}

#[derive(Clone, Debug)]
pub enum StaticVoxelServerEvent {
    ChunkLoaded {
        connection_id: ConnectionId,
        chunk: GeneratedChunk,
    },
    ChunkUnloaded {
        connection_id: ConnectionId,
        coordinate: ChunkCoordinate,
    },
}

#[derive(Resource)]
pub struct StaticVoxelServerWorld {
    generator: InfiniteSphereGenerator,
    loaded_chunks: HashMap<ChunkCoordinate, GeneratedChunk>,
    desired_by_character: HashMap<CharacterId, HashSet<ChunkCoordinate>>,
    subscriptions: HashMap<ConnectionId, CharacterSubscription>,
}

impl StaticVoxelServerWorld {
    fn new(generator: InfiniteSphereGenerator) -> Self {
        Self {
            generator,
            loaded_chunks: HashMap::new(),
            desired_by_character: HashMap::new(),
            subscriptions: HashMap::new(),
        }
    }

    pub fn chunk(&self, coordinate: ChunkCoordinate) -> Option<&GeneratedChunk> {
        self.loaded_chunks.get(&coordinate)
    }

    pub fn loaded_chunk_count(&self) -> usize {
        self.loaded_chunks.len()
    }
}

#[derive(Resource, Clone)]
struct StaticVoxelServerPipe(
    CrossbeamThreadPipeEndpointB<StaticVoxelServerCommand, StaticVoxelServerEvent>,
);

struct CharacterSubscription {
    character_id: CharacterId,
    sent_chunks: HashSet<ChunkCoordinate>,
}

fn maintain_character_chunks(
    pipe: Res<StaticVoxelServerPipe>,
    mut world: ResMut<StaticVoxelServerWorld>,
    characters: Query<(&Character, &Transform), With<ServerCharacter>>,
) {
    while let Some(command) = pipe.0.try_receive() {
        let StaticVoxelServerCommand::SubscribeCharacter {
            connection_id,
            character_id,
        } = command;
        world.subscriptions.insert(
            connection_id,
            CharacterSubscription {
                character_id,
                sent_chunks: HashSet::new(),
            },
        );
    }

    world.desired_by_character.clear();
    for (character, transform) in &characters {
        world.desired_by_character.insert(
            character.id,
            chunks_intersecting_radius(
                transform.translation.as_dvec3().to_array(),
                CHARACTER_CHUNK_LOAD_RADIUS,
            ),
        );
    }

    let desired_world_chunks = world
        .desired_by_character
        .values()
        .flat_map(|coordinates| coordinates.iter().copied())
        .collect::<HashSet<_>>();
    world
        .loaded_chunks
        .retain(|coordinate, _| desired_world_chunks.contains(coordinate));

    let mut missing = desired_world_chunks
        .iter()
        .filter(|coordinate| !world.loaded_chunks.contains_key(*coordinate))
        .copied()
        .collect::<Vec<_>>();
    missing.sort_unstable();
    for coordinate in missing.into_iter().take(MAX_CHUNKS_GENERATED_PER_TICK) {
        let chunk = world
            .generator
            .generate_chunk(coordinate[0], coordinate[1], coordinate[2]);
        world.loaded_chunks.insert(coordinate, chunk);
    }

    publish_subscription_changes(&pipe.0, &mut world);
}

fn publish_subscription_changes(
    pipe: &CrossbeamThreadPipeEndpointB<StaticVoxelServerCommand, StaticVoxelServerEvent>,
    world: &mut StaticVoxelServerWorld,
) {
    let subscription_ids = world.subscriptions.keys().copied().collect::<Vec<_>>();
    for connection_id in subscription_ids {
        let Some(character_id) = world
            .subscriptions
            .get(&connection_id)
            .map(|subscription| subscription.character_id)
        else {
            continue;
        };
        let desired = world
            .desired_by_character
            .get(&character_id)
            .cloned()
            .unwrap_or_default();
        let available = desired
            .iter()
            .filter(|coordinate| world.loaded_chunks.contains_key(*coordinate))
            .copied()
            .collect::<HashSet<_>>();
        let sent = &world.subscriptions[&connection_id].sent_chunks;
        let added = available.difference(sent).copied().collect::<Vec<_>>();
        let removed = sent.difference(&desired).copied().collect::<Vec<_>>();

        for coordinate in &added {
            let _ = pipe.try_send(StaticVoxelServerEvent::ChunkLoaded {
                connection_id,
                chunk: world.loaded_chunks[coordinate].clone(),
            });
        }
        for coordinate in &removed {
            let _ = pipe.try_send(StaticVoxelServerEvent::ChunkUnloaded {
                connection_id,
                coordinate: *coordinate,
            });
        }

        if let Some(subscription) = world.subscriptions.get_mut(&connection_id) {
            subscription.sent_chunks.extend(added);
            for coordinate in removed {
                subscription.sent_chunks.remove(&coordinate);
            }
        }
    }
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
