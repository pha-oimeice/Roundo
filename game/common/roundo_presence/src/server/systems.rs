//! Authoritative presence command processing and snapshot publication.

use super::*;

// Command ingestion precedes change publication in the same fixed tick.
impl Plugin for RoundoPresenceServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PresenceServerSettings>()
            .init_resource::<PlayerRegistry>()
            .insert_resource(PresenceServerPipe(self.pipe.endpoint_b()))
            .configure_sets(
                FixedUpdate,
                (
                    PresenceServerSet::Commands,
                    PresenceServerSet::PlayerStates,
                    PresenceServerSet::Snapshots,
                )
                    .chain(),
            )
            .add_systems(
                FixedUpdate,
                process_presence_commands.in_set(PresenceServerSet::Commands),
            )
            .add_systems(
                FixedUpdate,
                publish_changed_player_states.in_set(PresenceServerSet::PlayerStates),
            )
            .add_systems(
                FixedUpdate,
                publish_presence_snapshots.in_set(PresenceServerSet::Snapshots),
            );
    }
}

#[derive(Resource, Clone)]
/// ECS-side endpoint for network commands and presence events.
struct PresenceServerPipe(CrossbeamThreadPipeEndpointB<PresenceServerCommand, PresenceServerEvent>);

#[derive(Clone, Copy)]
/// Registry metadata required to route one connected player.
struct PlayerEntry {
    entity: Entity,
    player_id: PlayerId,
}

#[derive(Resource)]
/// Index from connections to player identities and ECS entities.
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

// Applies connection and disconnection commands from the network bridge.
fn process_presence_commands(
    mut commands: Commands,
    pipe: Res<PresenceServerPipe>,
    mut registry: ResMut<PlayerRegistry>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            // New players receive a deterministic initial transform.
            PresenceServerCommand::Connect { connection_id } => {
                if registry.by_connection.contains_key(&connection_id) {
                    continue;
                }
                let player_id = PlayerId(registry.next_player_id);
                registry.next_player_id = registry.next_player_id.saturating_add(1);
                let transform = s1_spawn_transform();
                let scene = PlayerScene {
                    scene_id: SceneId::S1,
                };
                let entity = commands
                    .spawn((
                        Player { id: player_id },
                        ServerPlayer,
                        PlayerConnection(connection_id),
                        scene,
                        transform,
                    ))
                    .id();
                registry
                    .by_connection
                    .insert(connection_id, PlayerEntry { entity, player_id });
                if pipe
                    .0
                    .try_send(PresenceServerEvent::PlayerJoined {
                        connection_id,
                        player_id,
                    })
                    .is_err()
                {
                    log::error!(
                        "cannot publish Presence player join: connection_id={}, player_id={}, reason=server_bridge_closed",
                        connection_id.0,
                        player_id.0
                    );
                }
            }
            // Disconnect removes only the entity owned by that connection.
            PresenceServerCommand::Disconnect { connection_id } => {
                if let Some(entry) = registry.by_connection.remove(&connection_id) {
                    commands.entity(entry.entity).despawn();
                }
            }
        }
    }
}

// Publishes transforms only when Bevy change detection marks them modified.
fn publish_changed_player_states(
    pipe: Res<PresenceServerPipe>,
    registry: Res<PlayerRegistry>,
    players: Query<(&Transform, &PlayerScene), (With<ServerPlayer>, Changed<Transform>)>,
) {
    for (connection_id, entry) in &registry.by_connection {
        let Ok((transform, scene)) = players.get(entry.entity) else {
            continue;
        };
        if pipe
            .0
            .try_send(PresenceServerEvent::PlayerStateChanged {
                connection_id: *connection_id,
                state: player_state(entry.player_id, *scene, transform),
            })
            .is_err()
        {
            log::error!(
                "cannot publish Presence player state: connection_id={}, player_id={}, reason=server_bridge_closed",
                connection_id.0,
                entry.player_id.0
            );
        }
    }
}

// Builds per-player nearby snapshots within the configured world radius.
fn publish_presence_snapshots(
    pipe: Res<PresenceServerPipe>,
    settings: Res<PresenceServerSettings>,
    registry: Res<PlayerRegistry>,
    players: Query<(&Player, &Transform, &PlayerScene), With<ServerPlayer>>,
) {
    let radius_squared = settings.radius * settings.radius;
    for (connection_id, entry) in &registry.by_connection {
        let Ok((_, own_transform, own_scene)) = players.get(entry.entity) else {
            continue;
        };
        let mut nearby_players = players
            .iter()
            .filter(|(_, transform, scene)| {
                scene.scene_id == own_scene.scene_id
                    && own_transform
                        .translation
                        .distance_squared(transform.translation)
                        <= radius_squared
            })
            .map(|(player, transform, _)| NearbyPlayer {
                player_id: player.id,
                translation: transform.translation.to_array(),
                rotation: transform.rotation.to_array(),
            })
            .collect::<Vec<_>>();
        nearby_players.sort_unstable_by_key(|player| player.player_id.0);

        if pipe
            .0
            .try_send(PresenceServerEvent::Snapshot {
                connection_id: *connection_id,
                snapshot: PresenceSnapshot {
                    own_player_id: entry.player_id,
                    players: nearby_players,
                    joinable_worlds: Vec::new(),
                },
            })
            .is_err()
        {
            log::error!(
                "cannot publish Presence snapshot: connection_id={}, player_id={}, reason=server_bridge_closed",
                connection_id.0,
                entry.player_id.0
            );
        }
    }
}

// Converts engine transforms into stable wire arrays.
fn player_state(player_id: PlayerId, scene: PlayerScene, transform: &Transform) -> PlayerState {
    PlayerState {
        player_id,
        scene_id: scene.scene_id,
        translation: transform.translation.to_array(),
        rotation: transform.rotation.to_array(),
    }
}

// Scene S1 uses a fixed spawn pose facing negative Y.
fn s1_spawn_transform() -> Transform {
    Transform {
        translation: S1_SPAWN_TRANSLATION,
        rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
