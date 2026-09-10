use super::*;
use crate::character_controller::{
    block_interaction::route_block_interactions,
    movement::{movement_world_direction, route_player_movement},
    rotation::{
        ClientRotationSyncState, apply_camera_rotation, rotate_camera, synchronize_rotation,
    },
};
use crate::{
    ControllerCommand, DestroyBlockControllerAction, Movement3DAction, PlaceBlockControllerAction,
    PlayerControllerCommand, ROTATION_SYNC_INTERVAL_SECS, RotationSync,
};
use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::{
        App, ButtonInput, Camera, EulerRot, MouseButton, Quat, Time, Transform, Update, Vec2, Vec3,
    },
};
use roundo_contracts::{PlayerId, SceneId};
use roundo_toolbox::CrossbeamThreadPipe;
use std::time::Duration;

fn player_state(translation: [f32; 3], rotation: Quat) -> PlayerState {
    PlayerState {
        player_id: PlayerId(1),
        scene_id: SceneId::S1,
        translation,
        rotation: rotation.to_array(),
    }
}

#[test]
fn later_player_states_retarget_translation_without_overriding_local_rotation() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<AuthoritativePlayerState>()
        .init_resource::<PlayerTranslationInterpolation>()
        .init_resource::<ClientControllerCommandState>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_systems(
            Update,
            (interpolate_player_translation, process_server_commands).chain(),
        );
    let initial_rotation = Quat::from_rotation_y(0.2);
    let local_rotation = Quat::from_rotation_y(0.6);
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::default()))
        .id();
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .bind_camera(camera);

    commands
        .try_send(ClientMarionetteCommand::PlayerState(player_state(
            [2.0, 0.0, 0.0],
            initial_rotation,
        )))
        .unwrap();
    app.update();
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .rotation = local_rotation;
    commands
        .try_send(ClientMarionetteCommand::PlayerState(player_state(
            [12.0, 0.0, 0.0],
            Quat::from_rotation_y(1.0),
        )))
        .unwrap();
    app.update();

    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(
            PLAYER_INTERPOLATION_DURATION_SECS / 2.0,
        ));
    app.update();

    let transform = app.world().get::<Transform>(camera).unwrap();
    assert_eq!(transform.translation, Vec3::new(7.0, 0.0, 0.0));
    assert!(transform.rotation.abs_diff_eq(local_rotation, f32::EPSILON));
}

#[test]
fn f1_toggles_spirit_walking_and_restores_the_authoritative_pose() {
    let authoritative_state = player_state([3.0, 4.0, 5.0], Quat::from_rotation_y(0.75));
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<PlayerTranslationInterpolation>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(AuthoritativePlayerState(Some(authoritative_state)))
        .add_systems(Update, toggle_spirit_walking);
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::default()))
        .id();
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .bind_camera(camera);

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F1);
    app.update();
    assert!(
        app.world()
            .resource::<ClientPlayerController>()
            .is_spirit_walking()
    );

    app.world_mut()
        .entity_mut(camera)
        .insert(Transform::from_xyz(100.0, 100.0, 100.0));
    {
        let mut keyboard = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keyboard.release(KeyCode::F1);
        keyboard.clear_just_pressed(KeyCode::F1);
    }
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F1);
    app.update();

    let controller = app.world().resource::<ClientPlayerController>();
    let transform = app.world().get::<Transform>(camera).unwrap();
    assert!(!controller.is_spirit_walking());
    assert_eq!(
        transform.translation,
        Vec3::from_array(authoritative_state.translation)
    );
    assert!(
        transform
            .rotation
            .abs_diff_eq(Quat::from_array(authoritative_state.rotation), f32::EPSILON)
    );
}

#[test]
fn movement_vector_uses_only_camera_horizontal_heading() {
    let bindings = ClientKeyBindings::default();
    let yaw = 0.4;
    let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, yaw, 0.7, 0.2));
    let mut keyboard = ButtonInput::default();
    keyboard.press(KeyCode::KeyW);

    let forward = movement_world_direction(&keyboard, &bindings, &transform);
    let expected = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;

    assert!(forward.abs_diff_eq(expected, f32::EPSILON));
    assert_eq!(forward.y, 0.0);
}

#[test]
fn vertical_movement_ignores_camera_rotation() {
    let bindings = ClientKeyBindings::default();
    let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.4, 0.7, 0.2));
    let mut keyboard = ButtonInput::default();
    keyboard.press(KeyCode::Space);
    assert_eq!(
        movement_world_direction(&keyboard, &bindings, &transform),
        Vec3::Y
    );

    keyboard.release(KeyCode::Space);
    keyboard.press(KeyCode::ShiftLeft);
    assert_eq!(
        movement_world_direction(&keyboard, &bindings, &transform),
        Vec3::NEG_Y
    );
}

#[test]
fn camera_pitch_stays_strictly_between_vertical_limits() {
    let mut upward = Transform::default();
    apply_camera_rotation(&mut upward, 0.0, f32::MAX);
    let (_, upward_pitch, _) = upward.rotation.to_euler(EulerRot::YXZ);

    let mut downward = Transform::default();
    apply_camera_rotation(&mut downward, 0.0, -f32::MAX);
    let (_, downward_pitch, _) = downward.rotation.to_euler(EulerRot::YXZ);

    assert!(upward_pitch < std::f32::consts::FRAC_PI_2);
    assert!(downward_pitch > -std::f32::consts::FRAC_PI_2);
}

#[test]
fn bindings_form_an_ordered_unique_bipartite_graph() {
    let mut bindings = ClientKeyBindings::default();
    assert!(!bindings.bind(KeyCode::KeyW, MovementAction::MoveForward));
    assert!(bindings.bind(KeyCode::KeyW, MovementAction::MoveUp));
    assert_eq!(
        bindings.actions_for(KeyCode::KeyW),
        &[MovementAction::MoveForward, MovementAction::MoveUp]
    );
    assert!(bindings.reorder(
        KeyCode::KeyW,
        MovementAction::MoveUp,
        MovementAction::MoveForward
    ));
    assert!(bindings.unbind(KeyCode::KeyW, MovementAction::MoveUp));
}

#[test]
fn rotation_is_immediate_and_network_sync_is_rate_limited() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<AccumulatedMouseMotion>()
        .init_resource::<Time>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteInputSettings>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [1.0, 2.0, 3.0],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_systems(Update, (rotate_camera, synchronize_rotation).chain());
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::from_xyz(1.0, 2.0, 3.0)))
        .id();
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .bind_camera(camera);
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::new(20.0, -10.0);

    app.update();
    let first_rotation = app.world().get::<Transform>(camera).unwrap().rotation;
    assert_ne!(first_rotation, Quat::IDENTITY);
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::SyncRotation(RotationSync { rotation })
        )) if Quat::from_array(rotation).abs_diff_eq(first_rotation, f32::EPSILON)
    ));

    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::new(10.0, 0.0);
    app.update();
    assert_ne!(
        app.world().get::<Transform>(camera).unwrap().rotation,
        first_rotation
    );
    assert!(events.try_receive().is_none());

    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(ROTATION_SYNC_INTERVAL_SECS));
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::ZERO;
    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::SyncRotation(_)
        ))
    ));
}

#[test]
fn movement_is_uploaded_as_a_world_space_translation_delta() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ClientKeyBindings>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteInputSettings>()
        .init_resource::<ClientControllerCommandState>()
        .init_resource::<Time>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_systems(Update, route_player_movement);
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::default()))
        .id();
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .bind_camera(camera);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(0.1));

    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::Movement3D(ControllerCommand {
                sequence: 1,
                action: Movement3DAction { translation_delta },
            })
        )) if Vec3::from_array(translation_delta).abs_diff_eq(Vec3::NEG_Z * 0.5, f32::EPSILON)
    ));

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyW);
    app.update();
    assert!(events.try_receive().is_none());
}

#[test]
fn mouse_buttons_upload_destroy_and_fixed_id_place_events() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientControllerCommandState>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_systems(Update, route_block_interactions);

    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::DestroyBlock(ControllerCommand {
                sequence: 1,
                action: DestroyBlockControllerAction,
            })
        ))
    ));

    {
        let mut buttons = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        buttons.release(MouseButton::Left);
        buttons.clear_just_pressed(MouseButton::Left);
        buttons.press(MouseButton::Right);
    }
    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::PlaceBlock(ControllerCommand {
                sequence: 1,
                action: PlaceBlockControllerAction { voxel_id: 1 },
            })
        ))
    ));
}
