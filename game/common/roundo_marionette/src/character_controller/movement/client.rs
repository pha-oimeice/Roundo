//! Converts registered input slots into world-space movement intent.

use super::Movement3DAction;
use crate::character_controller::{
    ClientInputBindings, MOVE_BACKWARD_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT, MOVE_FORWARD_INPUT_SLOT,
    MOVE_LEFT_INPUT_SLOT, MOVE_RIGHT_INPUT_SLOT, MOVE_UP_INPUT_SLOT,
    client::{
        AuthoritativePlayerState, ClientControllerCommandState, ClientMarionetteEvent,
        ClientMarionetteInputSettings, ClientMovementPredictionState, ClientPipeResource,
        ClientPlayerController,
    },
};
use bevy::prelude::{
    ButtonInput, Camera, EulerRot, KeyCode, MouseButton, Quat, Query, Res, ResMut, Time, Transform,
    Vec3, With,
};
use roundo_contracts::PlayerControllerCommand;

/// Sends a sequenced direction change; authoritative speed remains server-owned.
pub(crate) fn route_player_movement(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<ClientInputBindings>,
    settings: Res<ClientMarionetteInputSettings>,
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
    mut prediction: ResMut<ClientMovementPredictionState>,
    cameras: Query<&Transform, With<Camera>>,
) {
    if authoritative.0.is_none() {
        return;
    }
    let direction = if controller.input_enabled && !controller.spirit_walking {
        controller
            .camera
            .and_then(|camera| cameras.get(camera).ok())
            .map(|transform| movement_world_direction(&keyboard, &mouse, &bindings, transform))
            .unwrap_or(Vec3::ZERO)
    } else {
        Vec3::ZERO
    };
    if settings.player_movement_prediction && controller.input_enabled && !controller.spirit_walking
    {
        prediction.offset += direction * super::PLAYER_MOVE_SPEED * time.delta_secs();
    } else {
        prediction.offset = Vec3::ZERO;
    }
    if direction == controller_commands.movement.direction() {
        return;
    }
    let Ok(command) = controller_commands.movement.issue(Movement3DAction {
        direction: direction.to_array(),
    }) else {
        return;
    };
    if pipe
        .0
        .try_send(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::Movement3D(command),
        ))
        .is_err()
    {
        log::warn!("cannot route player movement input: reason=client_bridge_closed");
    }
}

/// Moves the detached spirit camera locally at the configured camera speed.
pub(crate) fn move_spirit_camera(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<ClientInputBindings>,
    settings: Res<ClientMarionetteInputSettings>,
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if !controller.input_enabled || !controller.spirit_walking {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };
    let Ok(mut transform) = cameras.get_mut(camera) else {
        return;
    };
    let direction = movement_world_direction(&keyboard, &mouse, &bindings, &transform);
    transform.translation += direction * settings.camera_move_speed * time.delta_secs();
}

/// Maps registered movement slots through the camera's horizontal heading.
pub(crate) fn movement_world_direction(
    keyboard: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    bindings: &ClientInputBindings,
    transform: &Transform,
) -> Vec3 {
    let axis = |positive, negative| {
        let positive = if bindings.pressed(positive, keyboard, mouse) {
            1.0
        } else {
            0.0
        };
        let negative = if bindings.pressed(negative, keyboard, mouse) {
            1.0
        } else {
            0.0
        };
        positive - negative
    };
    let local_direction = Vec3::new(
        axis(MOVE_RIGHT_INPUT_SLOT, MOVE_LEFT_INPUT_SLOT),
        axis(MOVE_UP_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT),
        axis(MOVE_FORWARD_INPUT_SLOT, MOVE_BACKWARD_INPUT_SLOT),
    )
    .normalize_or_zero();
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let horizontal_rotation = Quat::from_rotation_y(yaw);
    (horizontal_rotation * Vec3::X * local_direction.x
        + Vec3::Y * local_direction.y
        + horizontal_rotation * Vec3::NEG_Z * local_direction.z)
        .normalize_or_zero()
}
