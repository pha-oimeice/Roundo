use crate::character_controller::{
    ClientFunctionFrame, LOOK_FUNCTION,
    client::{ClientMarionetteInputSettings, ClientPlayerController},
};
use bevy::prelude::{Camera, EulerRot, MessageReader, Query, Res, Transform, With};

/// Applies accumulated mouse motion to the bound camera.
pub(crate) fn rotate_camera(
    mut inputs: MessageReader<ClientFunctionFrame>,
    settings: Res<ClientMarionetteInputSettings>,
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
    let delta = input.motion.get(LOOK_FUNCTION).copied().unwrap_or_default();
    let yaw_delta = -delta.x * settings.mouse_sensitivity;
    let pitch_delta = -delta.y * settings.mouse_sensitivity;
    apply_camera_rotation(&mut transform, yaw_delta, pitch_delta);
}

/// Applies finite yaw/pitch deltas while preventing vertical singularities.
pub(crate) fn apply_camera_rotation(
    transform: &mut Transform,
    yaw_delta: f32,
    pitch_delta: f32,
) -> bool {
    if !yaw_delta.is_finite()
        || !pitch_delta.is_finite()
        || (yaw_delta == 0.0 && pitch_delta == 0.0)
    {
        return false;
    }
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let pitch_limit = std::f32::consts::FRAC_PI_2 - 0.01;
    transform.rotation = bevy::prelude::Quat::from_euler(
        EulerRot::YXZ,
        yaw + yaw_delta,
        (pitch + pitch_delta).clamp(-pitch_limit, pitch_limit),
        0.0,
    );
    true
}
