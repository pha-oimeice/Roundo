//! Explicit adapters between independently owned server domain modules.

use bevy::prelude::{
    Added, App, Commands, Entity, FixedUpdate, IntoScheduleConfigs, Plugin, Query, With,
};
use roundo_marionette::{MarionetteServerSet, NetworkControllerTarget, PlayerControllers};
use roundo_presence::{PlayerConnection, PresenceServerSet, ServerPlayer};

/// Owns the narrow server-only seams between Presence, Marionette, and streaming.
///
/// Presence remains the authority that creates players. This plugin attaches
/// controller ownership before Marionette consumes commands. Chunk streaming
/// demand is owned independently by server-side loading anchors.
pub(crate) struct ServerDomainIntegrationPlugin;

impl Plugin for ServerDomainIntegrationPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            FixedUpdate,
            PresenceServerSet::Commands.before(MarionetteServerSet::Commands),
        )
        .add_systems(
            FixedUpdate,
            attach_player_control
                .after(PresenceServerSet::Commands)
                .before(MarionetteServerSet::Commands),
        );
    }
}

// Composes each newly added Presence player with connection-routed controllers.
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, Fixed, Time, Transform, Vec3};
    use roundo_marionette::{
        ConnectionId, ControllerCommand, MarionetteServerPlugin, Movement3DAction,
        PlayerControllerCommand, ServerMarionetteCommand,
    };
    use roundo_presence::{PresenceServerCommand, RoundoPresenceServerPlugin};
    use std::time::Duration;

    #[test]
    fn presence_players_are_composed_with_marionette_control() {
        let marionette = MarionetteServerPlugin::new();
        let control = marionette.ipc();
        let presence = RoundoPresenceServerPlugin::new();
        let lifecycle = presence.ipc();
        let connection_id = ConnectionId(13);
        let mut app = App::new();
        app.init_resource::<Time<Fixed>>().add_plugins((
            marionette,
            presence,
            ServerDomainIntegrationPlugin,
        ));

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
                        direction: [0.0, 0.0, 1.0],
                    },
                }),
            })
            .unwrap();
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(Duration::from_secs_f32(0.2));
        app.world_mut().run_schedule(FixedUpdate);

        let transform = app
            .world_mut()
            .query_filtered::<&Transform, With<ServerPlayer>>()
            .single(app.world())
            .unwrap();
        assert_eq!(transform.translation, Vec3::new(0.0, 2.0, 1.0));
    }
}
