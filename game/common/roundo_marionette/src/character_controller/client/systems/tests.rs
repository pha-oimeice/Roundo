use super::*;
#[cfg(feature = "dev")]
use crate::SpawnTestCreatureAction;
#[cfg(feature = "dev")]
use crate::character_controller::test_creature::route_spawn_test_creature;
use crate::character_controller::{
    block_interaction::route_block_interactions,
    movement::{movement_world_direction, route_player_movement},
    rotation::{
        ClientRotationSyncState, apply_camera_rotation, rotate_camera, synchronize_rotation,
    },
};
use crate::{
    ControllerCommand, DESTROY_BLOCK_INPUT_SLOT, DestroyBlockControllerAction, InputBinding,
    MOVE_BACKWARD_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT, MOVE_FORWARD_INPUT_SLOT, MOVE_LEFT_INPUT_SLOT,
    MOVE_RIGHT_INPUT_SLOT, MOVE_UP_INPUT_SLOT, Movement3DAction, PLACE_BLOCK_INPUT_SLOT,
    PhysicalInput, PlaceBlockControllerAction, PlayerControllerCommand,
    ROTATION_SYNC_INTERVAL_SECS, SPAWN_TEST_CREATURE_INPUT_SLOT,
};
use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::{
        App, ButtonInput, Camera, EulerRot, Messages, MouseButton, Quat, Time, Transform, Update,
        Vec2, Vec3,
    },
};
use roundo_contracts::{
    ControllerAccessLevel, ControllerControlState, ControllerId, ControllerIntentDomain,
    ControllerOperationError, PlayerControllerAccess, PlayerControllerAccessSnapshot,
    PlayerControllerOperation, PlayerId, SceneId,
};
use roundo_toolbox::CrossbeamThreadPipe;
use std::time::Duration;

fn default_bindings() -> ClientInputBindings {
    ClientInputBindings::new([
        InputBinding {
            slot: MOVE_UP_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::Space),
        },
        InputBinding {
            slot: MOVE_DOWN_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::ShiftLeft),
        },
        InputBinding {
            slot: MOVE_LEFT_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyA),
        },
        InputBinding {
            slot: MOVE_RIGHT_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyD),
        },
        InputBinding {
            slot: MOVE_FORWARD_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyW),
        },
        InputBinding {
            slot: MOVE_BACKWARD_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyS),
        },
        InputBinding {
            slot: SPIRIT_CAMERA_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::F1),
        },
        InputBinding {
            slot: DESTROY_BLOCK_INPUT_SLOT.into(),
            input: PhysicalInput::Mouse(MouseButton::Left),
        },
        InputBinding {
            slot: PLACE_BLOCK_INPUT_SLOT.into(),
            input: PhysicalInput::Mouse(MouseButton::Right),
        },
        InputBinding {
            slot: SPAWN_TEST_CREATURE_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::Digit1),
        },
    ])
}

fn player_state(translation: [f32; 3], rotation: Quat) -> PlayerState {
    PlayerState {
        player_id: PlayerId(1),
        scene_id: SceneId::S1,
        translation,
        rotation: rotation.to_array(),
    }
}

fn access_snapshot(control_state: ControllerControlState) -> PlayerControllerAccessSnapshot {
    PlayerControllerAccessSnapshot {
        player_id: PlayerId(1),
        controllers: vec![
            PlayerControllerAccess {
                controller_id: ControllerId(10),
                intent_domain: ControllerIntentDomain::Movement,
                access_level: ControllerAccessLevel::ReadWrite,
                control_state,
            },
            PlayerControllerAccess {
                controller_id: ControllerId(11),
                intent_domain: ControllerIntentDomain::Gaze,
                access_level: ControllerAccessLevel::ReadWrite,
                control_state,
            },
        ],
    }
}

#[test]
fn explicit_control_state_is_epoch_scoped_and_cleared_on_session_exit() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ClientPlayerController>()
        .init_resource::<ClientPlayerControllerAccess>()
        .init_resource::<ClientMarionetteSession>()
        .init_resource::<AuthoritativePlayerState>()
        .init_resource::<PlayerTranslationInterpolation>()
        .init_resource::<ClientControllerCommandState>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<CreatureAuthorityUpdate>()
        .add_systems(Update, process_server_commands);

    commands
        .try_send(ClientMarionetteCommand::BeginSession { epoch: 7 })
        .unwrap();
    app.update();
    assert!(matches!(
        commands.try_receive(),
        Some(ClientMarionetteEvent::RequestPlayerControllerAccess)
    ));

    commands
        .try_send(ClientMarionetteCommand::PlayerControllerAccessSnapshot {
            epoch: 6,
            snapshot: access_snapshot(ControllerControlState::ControlledBySelf),
        })
        .unwrap();
    app.update();
    assert!(
        app.world()
            .resource::<ClientPlayerControllerAccess>()
            .snapshot()
            .is_none()
    );

    commands
        .try_send(ClientMarionetteCommand::PlayerControllerAccessSnapshot {
            epoch: 7,
            snapshot: access_snapshot(ControllerControlState::Uncontrolled),
        })
        .unwrap();
    app.update();
    let access = app.world().resource::<ClientPlayerControllerAccess>();
    assert_eq!(access.movement_controller(), Some(ControllerId(10)));
    assert_eq!(access.gaze_controller(), Some(ControllerId(11)));
    assert!(!access.is_controlled_by_self(ControllerId(10)));

    commands
        .try_send(ClientMarionetteCommand::AcquireController {
            controller_id: ControllerId(10),
        })
        .unwrap();
    app.update();
    assert!(matches!(
        commands.try_receive(),
        Some(ClientMarionetteEvent::AcquireController {
            controller_id: ControllerId(10)
        })
    ));
    commands
        .try_send(ClientMarionetteCommand::ReleaseController {
            controller_id: ControllerId(10),
        })
        .unwrap();
    app.update();
    assert!(matches!(
        commands.try_receive(),
        Some(ClientMarionetteEvent::ReleaseController {
            controller_id: ControllerId(10)
        })
    ));
    commands
        .try_send(ClientMarionetteCommand::ControllerAcquired {
            epoch: 7,
            controller_id: ControllerId(10),
        })
        .unwrap();
    commands
        .try_send(ClientMarionetteCommand::ControllerOperationRejected {
            epoch: 7,
            operation: PlayerControllerOperation::Acquire {
                controller_id: ControllerId(11),
            },
            error: ControllerOperationError::ControlledByOther,
        })
        .unwrap();
    app.update();
    let access = app.world().resource::<ClientPlayerControllerAccess>();
    assert!(access.is_controlled_by_self(ControllerId(10)));
    assert_eq!(
        access.last_rejection(),
        Some((
            PlayerControllerOperation::Acquire {
                controller_id: ControllerId(11)
            },
            ControllerOperationError::ControlledByOther
        ))
    );

    commands
        .try_send(ClientMarionetteCommand::EndSession { epoch: 7 })
        .unwrap();
    commands
        .try_send(ClientMarionetteCommand::PlayerControllerAccessSnapshot {
            epoch: 7,
            snapshot: access_snapshot(ControllerControlState::ControlledBySelf),
        })
        .unwrap();
    app.update();
    assert_eq!(
        *app.world().resource::<ClientPlayerControllerAccess>(),
        ClientPlayerControllerAccess::default()
    );
}

#[test]
fn player_state_does_not_overwrite_the_local_camera_projection() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteSession>()
        .init_resource::<AuthoritativePlayerState>()
        .init_resource::<PlayerTranslationInterpolation>()
        .init_resource::<ClientControllerCommandState>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<CreatureAuthorityUpdate>()
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
        .try_send(ClientMarionetteCommand::BeginSession { epoch: 1 })
        .unwrap();
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
    assert_eq!(transform.translation, Vec3::ZERO);
    assert!(transform.rotation.abs_diff_eq(local_rotation, f32::EPSILON));
}

#[test]
fn every_new_session_starts_with_detached_control_and_no_old_authority() {
    let transport = CrossbeamThreadPipe::new();
    let commands = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteSession>()
        .init_resource::<AuthoritativePlayerState>()
        .init_resource::<PlayerTranslationInterpolation>()
        .init_resource::<ClientControllerCommandState>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<CreatureAuthorityUpdate>()
        .add_systems(Update, process_server_commands);
    {
        let mut controller = app.world_mut().resource_mut::<ClientPlayerController>();
        controller.spirit_walking = false;
    }
    app.world_mut().resource_mut::<AuthoritativePlayerState>().0 = Some(player_state(
        [9.0, 0.0, 0.0],
        Quat::from_rotation_y(std::f32::consts::PI),
    ));

    commands
        .try_send(ClientMarionetteCommand::BeginSession { epoch: 2 })
        .unwrap();
    app.update();

    assert!(
        app.world()
            .resource::<ClientPlayerController>()
            .is_spirit_walking()
    );
    assert!(
        app.world()
            .resource::<AuthoritativePlayerState>()
            .0
            .is_none()
    );
    assert_eq!(app.world().resource::<ClientMarionetteSession>().0, Some(2));
}

#[test]
fn f1_explicitly_binds_then_detaches_the_initial_free_camera() {
    let authoritative_state = player_state([3.0, 4.0, 5.0], Quat::from_rotation_y(0.75));
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(default_bindings())
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
        !app.world()
            .resource::<ClientPlayerController>()
            .is_spirit_walking()
    );
    let bound_transform = app.world().get::<Transform>(camera).unwrap();
    assert_eq!(
        bound_transform.translation,
        Vec3::from_array(authoritative_state.translation)
    );
    assert!(
        bound_transform
            .rotation
            .abs_diff_eq(Quat::from_array(authoritative_state.rotation), f32::EPSILON)
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
    assert!(controller.is_spirit_walking());
    assert_eq!(transform.translation, Vec3::splat(100.0));
}

#[cfg(feature = "dev")]
#[test]
fn registered_spawn_slot_emits_sequenced_test_creature_intent() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<ClientPlayerController>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(default_bindings())
        .init_resource::<ClientControllerCommandState>()
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_systems(Update, route_spawn_test_creature);
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .spirit_walking = false;

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Digit1);
    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::SpawnTestCreature(ControllerCommand {
                sequence: 1,
                action: SpawnTestCreatureAction,
            })
        ))
    ));

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear_just_pressed(KeyCode::Digit1);
    app.update();
    assert!(events.try_receive().is_none());
}

#[test]
fn movement_vector_uses_only_camera_horizontal_heading() {
    let bindings = default_bindings();
    let yaw = 0.4;
    let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, yaw, 0.7, 0.2));
    let mut keyboard = ButtonInput::default();
    let mouse = ButtonInput::default();
    keyboard.press(KeyCode::KeyW);

    let forward = movement_world_direction(&keyboard, &mouse, &bindings, &transform);
    let expected = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;

    assert!(forward.abs_diff_eq(expected, f32::EPSILON));
    assert_eq!(forward.y, 0.0);
}

#[test]
fn vertical_movement_ignores_camera_rotation() {
    let bindings = default_bindings();
    let transform = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.4, 0.7, 0.2));
    let mut keyboard = ButtonInput::default();
    let mouse = ButtonInput::default();
    keyboard.press(KeyCode::Space);
    assert_eq!(
        movement_world_direction(&keyboard, &mouse, &bindings, &transform),
        Vec3::Y
    );

    keyboard.release(KeyCode::Space);
    keyboard.press(KeyCode::ShiftLeft);
    assert_eq!(
        movement_world_direction(&keyboard, &mouse, &bindings, &transform),
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
fn bindings_support_many_to_many_edges() {
    let bindings = ClientInputBindings::new([
        InputBinding {
            slot: MOVE_FORWARD_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyW),
        },
        InputBinding {
            slot: MOVE_UP_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyW),
        },
        InputBinding {
            slot: MOVE_FORWARD_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::ArrowUp),
        },
    ]);
    assert_eq!(bindings.iter().count(), 3);
}

#[test]
fn detached_camera_rotation_never_updates_creature_gaze() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<AccumulatedMouseMotion>()
        .init_resource::<Time>()
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteInputSettings>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<LocallyRoutedControllerIntent>()
        .add_systems(Update, (rotate_camera, synchronize_rotation).chain());
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::default()))
        .id();
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .bind_camera(camera);
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::new(30.0, -10.0);
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(1.0));

    app.update();

    assert_ne!(
        app.world().get::<Transform>(camera).unwrap().rotation,
        Quat::IDENTITY
    );
    assert!(events.try_receive().is_none());
}

#[test]
fn rebinding_preserves_monotonic_creature_gaze_sequence() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<AccumulatedMouseMotion>()
        .init_resource::<Time>()
        .insert_resource(default_bindings())
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteInputSettings>()
        .init_resource::<PlayerTranslationInterpolation>()
        .init_resource::<ClientRotationSyncState>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<LocallyRoutedControllerIntent>()
        .add_systems(
            Update,
            (toggle_spirit_walking, rotate_camera, synchronize_rotation).chain(),
        );
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::default()))
        .id();
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .bind_camera(camera);

    // Initial free camera -> first binding.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F1);
    app.update();
    clear_f1(&mut app);
    send_mouse_gaze(&mut app, Vec2::new(20.0, 0.0));
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::Gaze(command)
        )) if command.sequence == 1
    ));

    // Detach, then bind the same controller again.
    press_f1(&mut app);
    clear_f1(&mut app);
    press_f1(&mut app);
    clear_f1(&mut app);
    send_mouse_gaze(&mut app, Vec2::new(20.0, 0.0));

    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::Gaze(command)
        )) if command.sequence == 2
    ));
}

fn press_f1(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F1);
    app.update();
}

fn clear_f1(app: &mut App) {
    let mut keyboard = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
    keyboard.release(KeyCode::F1);
    keyboard.clear_just_pressed(KeyCode::F1);
}

fn send_mouse_gaze(app: &mut App, delta: Vec2) {
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = delta;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(ROTATION_SYNC_INTERVAL_SECS));
    app.update();
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::ZERO;
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
        .insert_resource(ClientPlayerControllerAccess {
            snapshot: Some(access_snapshot(ControllerControlState::ControlledBySelf)),
            movement: Some(ControllerId(10)),
            gaze: Some(ControllerId(11)),
            last_rejection: None,
        })
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [1.0, 2.0, 3.0],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<LocallyRoutedControllerIntent>()
        .add_systems(Update, (rotate_camera, synchronize_rotation).chain());
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::from_xyz(1.0, 2.0, 3.0)))
        .id();
    {
        let mut controller = app.world_mut().resource_mut::<ClientPlayerController>();
        controller.bind_camera(camera);
        controller.spirit_walking = false;
    }
    app.world_mut()
        .resource_mut::<AccumulatedMouseMotion>()
        .delta = Vec2::new(20.0, -10.0);

    app.update();
    let first_rotation = app.world().get::<Transform>(camera).unwrap().rotation;
    assert_ne!(first_rotation, Quat::IDENTITY);
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::SubmitControllerInput {
            controller_id: ControllerId(11),
            input: roundo_contracts::DirectedControllerInput::Gaze(command),
        }) if command.sequence == 1 && command.action.yaw_delta.is_finite()
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
        Some(ClientMarionetteEvent::SubmitControllerInput {
            controller_id: ControllerId(11),
            input: roundo_contracts::DirectedControllerInput::Gaze(_),
        })
    ));
}

#[test]
fn uncontrolled_movement_is_neither_uploaded_nor_predicted() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(default_bindings())
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientControllerCommandState>()
        .insert_resource(ClientPlayerControllerAccess {
            snapshot: Some(access_snapshot(ControllerControlState::Uncontrolled)),
            movement: Some(ControllerId(10)),
            gaze: Some(ControllerId(11)),
            last_rejection: None,
        })
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<LocallyRoutedControllerIntent>()
        .add_systems(Update, route_player_movement);
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .spirit_walking = false;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    app.update();

    assert!(events.try_receive().is_none());
    let messages = app
        .world()
        .resource::<Messages<LocallyRoutedControllerIntent>>();
    assert_eq!(messages.len(), 0);
}

#[test]
fn controlled_movement_is_uploaded_to_its_explicit_controller() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(default_bindings())
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientMarionetteInputSettings>()
        .init_resource::<ClientControllerCommandState>()
        .insert_resource(ClientPlayerControllerAccess {
            snapshot: Some(access_snapshot(ControllerControlState::ControlledBySelf)),
            movement: Some(ControllerId(10)),
            gaze: Some(ControllerId(11)),
            last_rejection: None,
        })
        .init_resource::<Time>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_message::<LocallyRoutedControllerIntent>()
        .add_systems(Update, route_player_movement);
    let camera = app
        .world_mut()
        .spawn((Camera::default(), Transform::default()))
        .id();
    {
        let mut controller = app.world_mut().resource_mut::<ClientPlayerController>();
        controller.bind_camera(camera);
        controller.spirit_walking = false;
    }
    app.world_mut()
        .resource_mut::<ClientMarionetteInputSettings>()
        .player_movement_prediction = true;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(0.1));

    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::SubmitControllerInput {
            controller_id: ControllerId(10),
            input: roundo_contracts::DirectedControllerInput::Movement3D(ControllerCommand {
                sequence: 1,
                action: Movement3DAction { direction },
            }),
        }) if Vec3::from_array(direction).abs_diff_eq(Vec3::Z, f32::EPSILON)
    ));
    {
        let messages = app
            .world()
            .resource::<Messages<LocallyRoutedControllerIntent>>();
        let mut cursor = messages.get_cursor();
        assert!(matches!(
            cursor.read(messages).next(),
            Some(LocallyRoutedControllerIntent(
                PlayerControllerCommand::Movement3D(ControllerCommand {
                    sequence: 1,
                    action: Movement3DAction { direction },
                })
            )) if Vec3::from_array(*direction).abs_diff_eq(Vec3::Z, f32::EPSILON)
        ));
    }

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyW);
    app.update();
    assert!(matches!(
        events.try_receive(),
        Some(ClientMarionetteEvent::SubmitControllerInput {
            controller_id: ControllerId(10),
            input: roundo_contracts::DirectedControllerInput::Movement3D(ControllerCommand {
                sequence: 2,
                action: Movement3DAction { direction },
            }),
        }) if Vec3::from_array(direction) == Vec3::ZERO
    ));
}

#[test]
fn mouse_buttons_upload_destroy_and_fixed_id_place_events() {
    let transport = CrossbeamThreadPipe::new();
    let events = transport.endpoint_a();
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(default_bindings())
        .init_resource::<ClientPlayerController>()
        .init_resource::<ClientControllerCommandState>()
        .insert_resource(AuthoritativePlayerState(Some(player_state(
            [0.0; 3],
            Quat::IDENTITY,
        ))))
        .insert_resource(ClientPipeResource(transport.endpoint_b()))
        .add_systems(Update, route_block_interactions);
    app.world_mut()
        .resource_mut::<ClientPlayerController>()
        .spirit_walking = false;

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
