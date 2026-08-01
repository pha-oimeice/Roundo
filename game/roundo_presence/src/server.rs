use crate::{
    ARDA_WORLD_ID, ARDA_WORLD_NAME, JoinableWorld, NearbyJoinableWorld, NearbyPlayer, Player,
    PlayerId, PresenceSnapshot,
};
use bevy::prelude::{
    App, Commands, Component, Entity, FixedUpdate, IntoScheduleConfigs, Plugin, Query, Res,
    Resource, Startup, Transform, Vec3, With,
};
use roundo_networking::ConnectionId;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::HashMap;

pub const DEFAULT_PRESENCE_RADIUS: f32 = 128.0;

pub type PresenceServerIpc =
    CrossbeamThreadPipeEndpointA<PresenceServerCommand, PresenceServerEvent>;

#[derive(Clone)]
pub struct RoundoPresenceServerPlugin {
    pipe: CrossbeamThreadPipe<PresenceServerCommand, PresenceServerEvent>,
}

impl RoundoPresenceServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> PresenceServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for RoundoPresenceServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for RoundoPresenceServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PresenceServerSettings>()
            .init_resource::<PlayerRegistry>()
            .insert_resource(PresenceServerPipe(self.pipe.endpoint_b()))
            .add_systems(Startup, spawn_arda)
            .add_systems(
                FixedUpdate,
                (process_presence_commands, publish_presence_snapshots).chain(),
            );
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct PresenceServerSettings {
    pub radius: f32,
}

impl PresenceServerSettings {
    pub fn new(radius: f32) -> Self {
        Self {
            radius: valid_radius(radius),
        }
    }
}

impl Default for PresenceServerSettings {
    fn default() -> Self {
        Self::new(DEFAULT_PRESENCE_RADIUS)
    }
}

#[derive(Clone, Debug)]
pub enum PresenceServerCommand {
    Connect {
        connection_id: ConnectionId,
    },
    Disconnect {
        connection_id: ConnectionId,
    },
    UpdatePosition {
        connection_id: ConnectionId,
        translation: [f32; 3],
    },
}

#[derive(Clone, Debug)]
pub enum PresenceServerEvent {
    PlayerJoined {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    Snapshot {
        connection_id: ConnectionId,
        snapshot: PresenceSnapshot,
    },
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ServerPlayer;

#[derive(Component, Clone, Copy, Debug, Default)]
struct ServerJoinableWorld;

#[derive(Resource, Clone)]
struct PresenceServerPipe(CrossbeamThreadPipeEndpointB<PresenceServerCommand, PresenceServerEvent>);

#[derive(Clone, Copy)]
struct PlayerEntry {
    entity: Entity,
    player_id: PlayerId,
}

#[derive(Resource)]
struct PlayerRegistry {
    next_player_id: u64,
    by_connection: HashMap<ConnectionId, PlayerEntry>,
}

impl Default for PlayerRegistry {
    fn default() -> Self {
        Self {
            next_player_id: 1,
            by_connection: HashMap::new(),
        }
    }
}

fn spawn_arda(mut commands: Commands) {
    commands.spawn((
        JoinableWorld {
            id: ARDA_WORLD_ID,
            name: ARDA_WORLD_NAME.to_string(),
        },
        ServerJoinableWorld,
        Transform::default(),
    ));
}

fn process_presence_commands(
    mut commands: Commands,
    pipe: Res<PresenceServerPipe>,
    mut registry: bevy::prelude::ResMut<PlayerRegistry>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            PresenceServerCommand::Connect { connection_id } => {
                if registry.by_connection.contains_key(&connection_id) {
                    continue;
                }
                let player_id = PlayerId(registry.next_player_id);
                registry.next_player_id = registry.next_player_id.saturating_add(1);
                let entity = commands
                    .spawn((Player { id: player_id }, ServerPlayer, Transform::default()))
                    .id();
                registry
                    .by_connection
                    .insert(connection_id, PlayerEntry { entity, player_id });
                let _ = pipe.0.try_send(PresenceServerEvent::PlayerJoined {
                    connection_id,
                    player_id,
                });
            }
            PresenceServerCommand::Disconnect { connection_id } => {
                if let Some(entry) = registry.by_connection.remove(&connection_id) {
                    commands.entity(entry.entity).despawn();
                }
            }
            PresenceServerCommand::UpdatePosition {
                connection_id,
                translation,
            } => {
                if translation.iter().all(|value| value.is_finite())
                    && let Some(entry) = registry.by_connection.get(&connection_id)
                {
                    commands
                        .entity(entry.entity)
                        .insert(Transform::from_translation(Vec3::from_array(translation)));
                }
            }
        }
    }
}

fn publish_presence_snapshots(
    pipe: Res<PresenceServerPipe>,
    settings: Res<PresenceServerSettings>,
    registry: Res<PlayerRegistry>,
    players: Query<(&Player, &Transform), With<ServerPlayer>>,
    worlds: Query<(&JoinableWorld, &Transform), With<ServerJoinableWorld>>,
) {
    let radius_squared = settings.radius * settings.radius;
    for (connection_id, entry) in &registry.by_connection {
        let Ok((own_player, own_transform)) = players.get(entry.entity) else {
            continue;
        };
        let origin = own_transform.translation;
        let mut nearby_players = players
            .iter()
            .filter(|(player, transform)| {
                player.id != own_player.id
                    && within_radius(origin, transform.translation, radius_squared)
            })
            .map(|(player, transform)| NearbyPlayer {
                player_id: player.id,
                translation: transform.translation.to_array(),
            })
            .collect::<Vec<_>>();
        nearby_players.sort_unstable_by_key(|player| player.player_id.0);

        let mut joinable_worlds = worlds
            .iter()
            .filter(|(_, transform)| within_radius(origin, transform.translation, radius_squared))
            .map(|(world, transform)| NearbyJoinableWorld {
                world_id: world.id,
                name: world.name.clone(),
                translation: transform.translation.to_array(),
            })
            .collect::<Vec<_>>();
        joinable_worlds.sort_unstable_by_key(|world| world.world_id.0);

        let _ = pipe.0.try_send(PresenceServerEvent::Snapshot {
            connection_id: *connection_id,
            snapshot: PresenceSnapshot {
                own_player_id: entry.player_id,
                players: nearby_players,
                joinable_worlds,
            },
        });
    }
}

fn within_radius(origin: Vec3, candidate: Vec3, radius_squared: f32) -> bool {
    origin.distance_squared(candidate) <= radius_squared
}

fn valid_radius(radius: f32) -> f32 {
    if radius.is_finite() && radius > 0.0 {
        radius
    } else {
        DEFAULT_PRESENCE_RADIUS
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PRESENCE_RADIUS, PresenceServerSettings, within_radius};
    use bevy::prelude::Vec3;

    #[test]
    fn radius_boundary_is_inclusive() {
        assert!(within_radius(Vec3::ZERO, Vec3::X * 4.0, 16.0));
        assert!(!within_radius(Vec3::ZERO, Vec3::X * 4.01, 16.0));
    }

    #[test]
    fn invalid_radius_uses_default() {
        assert_eq!(
            PresenceServerSettings::new(f32::NAN).radius,
            DEFAULT_PRESENCE_RADIUS
        );
        assert_eq!(
            PresenceServerSettings::new(0.0).radius,
            DEFAULT_PRESENCE_RADIUS
        );
    }
}
