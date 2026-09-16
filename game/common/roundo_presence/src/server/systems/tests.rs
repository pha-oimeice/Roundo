use super::*;
use bevy::prelude::{Fixed, Time};

#[test]
fn anonymous_players_spawn_at_two_facing_negative_y() {
    let transform = s1_spawn_transform();
    assert_eq!(transform.translation, Vec3::new(0.0, 2.0, 0.0));
    assert!((transform.rotation * Vec3::NEG_Z).abs_diff_eq(Vec3::NEG_Y, f32::EPSILON * 4.0));
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
    assert!(app.world().get::<Player>(player_entity).is_some());
    assert!(app.world().get::<PlayerScene>(player_entity).is_some());
    assert!(app.world().get::<Transform>(player_entity).is_some());

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
    assert!(app.world().get::<ServerPlayer>(second).is_some());
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
