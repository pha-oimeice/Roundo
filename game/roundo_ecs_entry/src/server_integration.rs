//! Explicit adapters between independently owned server domain modules.

use bevy::prelude::{
    Added, App, Commands, Entity, FixedUpdate, GlobalTransform, IntoScheduleConfigs, Plugin, Query,
    Res, ResMut, Update, With,
};
use roundo_local_coordinate::{
    LocalCoordinateObservationInput, LocalCoordinateObserver, LocalCoordinateServerSet,
};
use roundo_marionette::{MarionetteServerSet, NetworkControllerTarget, PlayerControllers};
use roundo_presence::{
    Player, PlayerConnection, PlayerScene, PresenceServerSet, ServerPlayer, ServerSceneWorlds,
};
use std::collections::HashMap;

/// Owns server-only entity composition and schedule relations across domains.
pub(crate) struct ServerDomainIntegrationPlugin;

impl Plugin for ServerDomainIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            FixedUpdate,
            PresenceServerSet::Commands.before(MarionetteServerSet::Commands),
        )
        .configure_sets(
            FixedUpdate,
            PresenceServerSet::SceneConstraints.after(MarionetteServerSet::Movement),
        )
        .add_systems(
            FixedUpdate,
            attach_player_control
                .after(PresenceServerSet::Commands)
                .before(MarionetteServerSet::Commands),
        )
        .add_systems(
            Update,
            publish_local_coordinate_observations.before(LocalCoordinateServerSet::Prepare),
        );
    }
}

fn attach_player_control(
    mut commands: Commands,
    players: Query<(Entity, &PlayerConnection), (With<ServerPlayer>, Added<ServerPlayer>)>,
) {
    for (entity, connection) in &players {
        commands.entity(entity).insert((
            PlayerControllers::default(),
            NetworkControllerTarget {
                connection_id: connection.0,
            },
        ));
    }
}

fn publish_local_coordinate_observations(
    scenes: Res<ServerSceneWorlds>,
    players: Query<(&Player, &PlayerScene, &GlobalTransform), With<ServerPlayer>>,
    mut input: ResMut<LocalCoordinateObservationInput>,
) {
    let scene_extents = scenes
        .spaces()
        .map(|(scene_id, space)| (scene_id, space.size()))
        .collect::<HashMap<_, _>>();
    let observers = players
        .iter()
        .filter(|(_, scene, _)| scene_extents.contains_key(&scene.scene_id))
        .map(|(player, scene, transform)| LocalCoordinateObserver {
            player_id: player.id,
            scene_id: scene.scene_id,
            position: transform.translation().as_dvec3().to_array(),
        })
        .collect::<Vec<_>>();
    input.replace(observers, scene_extents);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, Fixed, Time, Transform, Vec3};
    use roundo_marionette::{
        ConnectionId, ControllerCommand, MarionetteServerPlugin, Movement3DAction,
        PlayerControllerCommand, ServerMarionetteCommand,
    };
    use roundo_presence::{PresenceServerCommand, RoundoPresenceServerPlugin};

    #[test]
    fn presence_players_are_composed_with_marionette_control() {
        let marionette = MarionetteServerPlugin::new();
        let control = marionette.ipc();
        let presence = RoundoPresenceServerPlugin::new();
        let lifecycle = presence.ipc();
        let connection_id = ConnectionId(13);
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>()
            .init_resource::<LocalCoordinateObservationInput>()
            .add_plugins((marionette, presence, ServerDomainIntegrationPlugin));

        lifecycle
            .try_send(PresenceServerCommand::Connect { connection_id })
            .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        control
            .try_send(ServerMarionetteCommand::UsePlayerController {
                connection_id,
                command: PlayerControllerCommand::Movement3D(ControllerCommand {
                    sequence: 1,
                    action: Movement3DAction {
                        translation_delta: [0.0, 0.0, 5.0],
                    },
                }),
            })
            .unwrap();
        app.world_mut().run_schedule(FixedUpdate);

        let transform = app
            .world_mut()
            .query_filtered::<&Transform, With<ServerPlayer>>()
            .single(app.world())
            .unwrap();
        assert_eq!(transform.translation, Vec3::new(0.0, 2.0, 5.0));
    }
}
