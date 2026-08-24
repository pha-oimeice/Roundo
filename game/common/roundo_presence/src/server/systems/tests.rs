use super::*;
use bevy::prelude::{Fixed, Time};
use roundo_marionette::{
    ControllerCommand, DestroyBlockController, MarionetteServerPlugin, Movement3D,
    Movement3DAction, PlaceBlockController, PlayerControllerCommand, ServerMarionetteCommand,
};

#[test]
fn s1_has_configurable_constant_default_dimensions() {
    let scenes = ServerSceneWorlds::default();
    assert_eq!(
        scenes.space(SceneId::S1).unwrap().size(),
        [DEFAULT_S1_EDGE_LENGTH; 3]
    );
}

#[test]
fn anonymous_players_spawn_at_two_facing_negative_y() {
    let transform = s1_spawn_transform();
    assert_eq!(transform.translation, Vec3::new(0.0, 2.0, 0.0));
    assert!((transform.rotation * Vec3::NEG_Z).abs_diff_eq(Vec3::NEG_Y, f32::EPSILON * 4.0));
}

#[test]
fn s0_room_dimensions_can_change_at_runtime() {
    let mut scenes = ServerSceneWorlds::default();
    assert!(scenes.resize_s0_room(7, DEFAULT_S0_ROOM_SIZE));
    assert_eq!(
        scenes.space(SceneId::S0 { room_id: 7 }).unwrap().size(),
        DEFAULT_S0_ROOM_SIZE
    );
    assert!(scenes.resize_s0_room(7, [64.0, 32.0, 16.0]));
    assert_eq!(
        scenes.space(SceneId::S0 { room_id: 7 }).unwrap().size(),
        [64.0, 32.0, 16.0]
    );
}

#[test]
fn torus_wraps_through_both_endpoints() {
    let space = TorusSpace::new([10.0; 3]).unwrap();
    assert_eq!(
        space.wrap(Vec3::new(-1.0, 10.0, 21.0)),
        Vec3::new(9.0, 0.0, 1.0)
    );
}

#[test]
fn torus_distance_uses_the_shortest_wrapped_path() {
    let space = TorusSpace::new([10.0; 3]).unwrap();
    assert_eq!(
        space.distance_squared(Vec3::new(0.5, 0.0, 0.0), Vec3::new(9.5, 0.0, 0.0)),
        1.0
    );
}

#[test]
fn changed_player_transforms_are_the_only_states_published() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let connection_id = ConnectionId(9);
    let player_id = PlayerId(3);
    let mut app = App::new();
    app.init_resource::<PlayerRegistry>()
        .insert_resource(PresenceServerPipe(transport.endpoint_b()))
        .add_systems(bevy::prelude::Update, publish_changed_player_states);
    let entity = app
        .world_mut()
        .spawn((
            ServerPlayer,
            PlayerScene {
                scene_id: SceneId::S1,
            },
            Transform::default(),
        ))
        .id();
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .by_connection
        .insert(connection_id, PlayerEntry { entity, player_id });

    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(PresenceServerEvent::PlayerStateChanged {
            connection_id: received_connection,
            state: PlayerState {
                player_id: received_player,
                ..
            },
        }) if received_connection == connection_id && received_player == player_id
    ));
    app.update();
    assert!(events.try_receive().is_none());

    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = Vec3::X;
    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(PresenceServerEvent::PlayerStateChanged {
            state: PlayerState {
                translation: [1.0, 0.0, 0.0],
                ..
            },
            ..
        })
    ));
}

#[test]
fn connecting_publishes_the_initial_server_player_transform() {
    let plugin = RoundoPresenceServerPlugin::new();
    let commands = plugin.ipc();
    let mut app = App::new();
    app.init_resource::<Time<Fixed>>().add_plugins(plugin);
    commands
        .try_send(PresenceServerCommand::Connect {
            connection_id: ConnectionId(12),
        })
        .unwrap();

    app.world_mut().run_schedule(FixedUpdate);

    let player_entity =
        app.world().resource::<PlayerRegistry>().by_connection[&ConnectionId(12)].entity;
    assert!(app.world().get::<Movement3D>(player_entity).is_some());
    assert!(
        app.world()
            .get::<DestroyBlockController>(player_entity)
            .is_some()
    );
    assert!(
        app.world()
            .get::<PlaceBlockController>(player_entity)
            .is_some()
    );

    let mut received_state = None;
    while let Some(event) = commands.try_receive() {
        if let PresenceServerEvent::PlayerStateChanged { state, .. } = event {
            received_state = Some(state);
        }
    }
    assert_eq!(
        received_state.map(|state| state.translation),
        Some(S1_SPAWN_TRANSLATION.to_array())
    );
}

#[test]
fn disconnecting_despawns_only_the_matching_player() {
    let plugin = RoundoPresenceServerPlugin::new();
    let commands = plugin.ipc();
    let first_connection = ConnectionId(21);
    let second_connection = ConnectionId(22);
    let mut app = App::new();
    app.init_resource::<Time<Fixed>>()
        .insert_resource(bevy::ecs::error::FallbackErrorHandler(
            bevy::ecs::error::panic,
        ))
        .add_plugins(plugin);
    commands
        .try_send(PresenceServerCommand::Connect {
            connection_id: first_connection,
        })
        .unwrap();
    commands
        .try_send(PresenceServerCommand::Connect {
            connection_id: second_connection,
        })
        .unwrap();
    app.world_mut().run_schedule(FixedUpdate);
    let first = app.world().resource::<PlayerRegistry>().by_connection[&first_connection].entity;
    let second = app.world().resource::<PlayerRegistry>().by_connection[&second_connection].entity;

    commands
        .try_send(PresenceServerCommand::Disconnect {
            connection_id: first_connection,
        })
        .unwrap();
    app.world_mut().run_schedule(FixedUpdate);

    assert!(app.world().get_entity(first).is_err());
    assert!(app.world().get::<Movement3D>(second).is_some());
    assert!(
        !app.world()
            .resource::<PlayerRegistry>()
            .by_connection
            .contains_key(&first_connection)
    );
    assert!(
        app.world()
            .resource::<PlayerRegistry>()
            .by_connection
            .contains_key(&second_connection)
    );
}

#[test]
fn player_controller_commands_drive_the_presence_player() {
    let marionette = MarionetteServerPlugin::new();
    let controller_commands = marionette.ipc();
    let presence = RoundoPresenceServerPlugin::new();
    let presence_commands = presence.ipc();
    let connection_id = ConnectionId(13);
    let mut app = App::new();
    app.init_resource::<Time<Fixed>>()
        .add_plugins((marionette, presence));
    presence_commands
        .try_send(PresenceServerCommand::Connect { connection_id })
        .unwrap();
    app.world_mut().run_schedule(FixedUpdate);

    controller_commands
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

    let player_entity =
        app.world().resource::<PlayerRegistry>().by_connection[&connection_id].entity;
    assert_eq!(
        app.world()
            .get::<Transform>(player_entity)
            .unwrap()
            .translation,
        Vec3::new(0.0, 2.0, 5.0)
    );
}
