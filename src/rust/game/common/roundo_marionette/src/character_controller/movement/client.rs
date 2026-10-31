//! Converts forwarded semantic input into Controller-local movement intent.

use super::Movement3DAction;
use crate::character_controller::{
    ClientInputFrame, MOVE_BACKWARD_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT, MOVE_FORWARD_INPUT_SLOT,
    MOVE_LEFT_INPUT_SLOT, MOVE_RIGHT_INPUT_SLOT, MOVE_UP_INPUT_SLOT,
    client::{
        ClientControllerCommandState, ClientMarionetteEvent, ClientMarionetteInputSettings,
        ClientPipeResource, ClientPlayerController, ClientPlayerControllerAccess,
        LocallyRoutedControllerIntent,
    },
};
use bevy::prelude::{
    Camera, EulerRot, MessageReader, MessageWriter, Quat, Query, Res, ResMut, Time, Transform,
    Vec3, With,
};
use roundo_contracts::PlayerControllerCommand;

/// Sends a sequenced direction change; authoritative speed remains server-owned.
pub(crate) fn route_player_movement(
    mut inputs: MessageReader<ClientInputFrame>,
    controller: Res<ClientPlayerController>,
    access: Res<ClientPlayerControllerAccess>,
    pipe: Res<ClientPipeResource>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
    mut routed_intents: MessageWriter<LocallyRoutedControllerIntent>,
) {
    let input = inputs.read().last().cloned().unwrap_or_default();
    let Some(controller_id) =
        access.producing_controller(roundo_contracts::ControllerIntentDomain::Movement)
    else {
        return;
    };
    let direction = if controller.input_enabled && !controller.spirit_walking {
        movement_local_axis(&input)
    } else {
        Vec3::ZERO
    };
    if direction == controller_commands.movement.direction() {
        return;
    }
    let Ok(command) = controller_commands.movement.issue(Movement3DAction {
        direction: direction.to_array(),
    }) else {
        return;
    };
    let routed = PlayerControllerCommand::Movement3D(command);
    if pipe
        .0
        .try_send(ClientMarionetteEvent::SubmitControllerInput {
            controller_id,
            input: roundo_contracts::DirectedControllerInput::Movement3D(command),
        })
        .is_err()
    {
        log::warn!("cannot route player movement input: reason=client_bridge_closed");
    } else {
        routed_intents.write(LocallyRoutedControllerIntent(routed));
    }
}

/// Moves the detached spirit camera locally at the configured camera speed.
pub(crate) fn move_spirit_camera(
    mut inputs: MessageReader<ClientInputFrame>,
    settings: Res<ClientMarionetteInputSettings>,
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    let input = inputs.read().last().cloned().unwrap_or_default();
    if !controller.input_enabled || !controller.spirit_walking {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };
    let Ok(mut transform) = cameras.get_mut(camera) else {
        return;
    };
    let direction = movement_world_direction(&input, &transform);
    transform.translation += direction * settings.camera_move_speed * time.delta_secs();
}

/// Maps registered movement slots through the camera's horizontal heading.
pub(crate) fn movement_world_direction(input: &ClientInputFrame, transform: &Transform) -> Vec3 {
    let local_direction = movement_local_axis(input);
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let horizontal_rotation = Quat::from_rotation_y(yaw);
    (horizontal_rotation * Vec3::X * local_direction.x
        + Vec3::Y * local_direction.y
        + horizontal_rotation * Vec3::NEG_Z * local_direction.z)
        .normalize_or_zero()
}

/// Controller-local axis; consumers never query a physical device.
pub(crate) fn movement_local_axis(input: &ClientInputFrame) -> Vec3 {
    let axis = |positive, negative| {
        f32::from(input.pressed(positive)) - f32::from(input.pressed(negative))
    };
    Vec3::new(
        axis(MOVE_RIGHT_INPUT_SLOT, MOVE_LEFT_INPUT_SLOT),
        axis(MOVE_UP_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT),
        axis(MOVE_FORWARD_INPUT_SLOT, MOVE_BACKWARD_INPUT_SLOT),
    )
    .normalize_or_zero()
}
