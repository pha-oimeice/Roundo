use super::{ClientKeyBindings, Movement3DAction};
use crate::character_controller::client::{
    AuthoritativePlayerState, ClientControllerCommandState, ClientMarionetteEvent,
    ClientMarionetteInputSettings, ClientPipeResource, ClientPlayerController,
};
use bevy::prelude::{
    ButtonInput, Camera, EulerRot, KeyCode, Quat, Query, Res, ResMut, Time, Transform, Vec3, With,
};
use roundo_contracts::PlayerControllerCommand;

pub(crate) fn route_player_movement(
    keyboard: Res<ButtonInput<KeyCode>>,
    bindings: Res<ClientKeyBindings>,
    settings: Res<ClientMarionetteInputSettings>,
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
    cameras: Query<&Transform, With<Camera>>,
) {
    if !controller.input_enabled || controller.spirit_walking || authoritative.0.is_none() {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };
    let Ok(transform) = cameras.get(camera) else {
        return;
    };
    let translation_delta = movement_world_direction(&keyboard, &bindings, transform)
        * settings.camera_move_speed
        * time.delta_secs();
    if translation_delta == Vec3::ZERO {
        return;
    }
    let Ok(command) = controller_commands.movement.issue(Movement3DAction {
        translation_delta: translation_delta.to_array(),
    }) else {
        return;
    };
    let _ = pipe.0.try_send(ClientMarionetteEvent::UsePlayerController(
        PlayerControllerCommand::Movement3D(command),
    ));
}

pub(crate) fn move_spirit_camera(
    keyboard: Res<ButtonInput<KeyCode>>,
    bindings: Res<ClientKeyBindings>,
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
    let direction = movement_world_direction(&keyboard, &bindings, &transform);
    transform.translation += direction * settings.camera_move_speed * time.delta_secs();
}

pub(crate) fn movement_world_direction(
    keyboard: &ButtonInput<KeyCode>,
    bindings: &ClientKeyBindings,
    transform: &Transform,
) -> Vec3 {
    let local_direction = bindings.movement_direction(keyboard).normalize_or_zero();
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let horizontal_rotation = Quat::from_rotation_y(yaw);
    (horizontal_rotation * Vec3::X * local_direction.x
        + Vec3::Y * local_direction.y
        + horizontal_rotation * Vec3::NEG_Z * local_direction.z)
        .normalize_or_zero()
}
