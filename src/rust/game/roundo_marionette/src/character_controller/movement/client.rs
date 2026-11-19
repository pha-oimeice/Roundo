use crate::character_controller::{
    ClientFunctionFrame, MOVE_BACKWARD_FUNCTION, MOVE_DOWN_FUNCTION, MOVE_FORWARD_FUNCTION,
    MOVE_LEFT_FUNCTION, MOVE_RIGHT_FUNCTION, MOVE_UP_FUNCTION,
    client::{ClientMarionetteInputSettings, ClientPlayerController},
};
use bevy::prelude::{
    Camera, EulerRot, MessageReader, Quat, Query, Res, Time, Transform, Vec3, With,
};

/// Moves the free camera locally at the configured camera speed.
pub(crate) fn move_free_camera(
    mut inputs: MessageReader<ClientFunctionFrame>,
    settings: Res<ClientMarionetteInputSettings>,
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    let input = inputs.read().last().cloned().unwrap_or_default();
    if !controller.input_enabled {
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

/// Maps named movement functions through the camera's horizontal heading.
pub(crate) fn movement_world_direction(
    input: &ClientFunctionFrame,
    transform: &Transform,
) -> Vec3 {
    let local_direction = movement_local_axis(input);
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let horizontal_rotation = Quat::from_rotation_y(yaw);
    (horizontal_rotation * Vec3::X * local_direction.x
        + Vec3::Y * local_direction.y
        + horizontal_rotation * Vec3::NEG_Z * local_direction.z)
        .normalize_or_zero()
}

/// Controller-local axis; consumers never query a physical device.
pub(crate) fn movement_local_axis(input: &ClientFunctionFrame) -> Vec3 {
    let axis = |positive, negative| {
        f32::from(input.pressed(positive)) - f32::from(input.pressed(negative))
    };
    Vec3::new(
        axis(MOVE_RIGHT_FUNCTION, MOVE_LEFT_FUNCTION),
        axis(MOVE_UP_FUNCTION, MOVE_DOWN_FUNCTION),
        axis(MOVE_FORWARD_FUNCTION, MOVE_BACKWARD_FUNCTION),
    )
    .normalize_or_zero()
}
