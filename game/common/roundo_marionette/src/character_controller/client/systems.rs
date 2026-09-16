//! Client controller scheduling and authoritative-state reconciliation.

use super::*;
use crate::character_controller::{
    ClientInputBindings, SPIRIT_CAMERA_INPUT_SLOT,
    block_interaction::route_block_interactions,
    movement::{move_spirit_camera, route_player_movement},
    rotation::{ClientRotationSyncState, rotate_camera, synchronize_rotation},
};
use bevy::prelude::{
    ButtonInput, Camera, IntoScheduleConfigs, KeyCode, MouseButton, Query, Res, ResMut, Time,
    Transform, Update, Vec3, With,
};

// Ordering is intentional: reconciliation precedes local input and upload.
impl Plugin for MarionetteClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientPlayerController>()
            .init_resource::<ClientMarionetteInputSettings>()
            .init_resource::<ClientInputBindings>()
            .init_resource::<AuthoritativePlayerState>()
            .init_resource::<PlayerTranslationInterpolation>()
            .init_resource::<ClientMovementPredictionState>()
            .init_resource::<ClientControllerCommandState>()
            .init_resource::<ClientRotationSyncState>()
            .insert_resource(ClientPipeResource(self.pipe.endpoint_b()))
            .add_systems(
                Update,
                (
                    interpolate_player_translation,
                    process_server_commands,
                    toggle_spirit_walking,
                    rotate_camera,
                    move_spirit_camera,
                    route_player_movement,
                    route_block_interactions,
                    synchronize_rotation,
                )
                    .chain(),
            );
    }
}

// Applies authoritative snapshots and resets all prediction state on teardown.
fn process_server_commands(
    pipe: Res<ClientPipeResource>,
    mut authoritative: ResMut<AuthoritativePlayerState>,
    mut controller: ResMut<ClientPlayerController>,
    mut translation_interpolation: ResMut<PlayerTranslationInterpolation>,
    mut movement_prediction: ResMut<ClientMovementPredictionState>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientMarionetteCommand::PlayerState(state) => {
                let was_uninitialized = authoritative.0.is_none();
                authoritative.0 = Some(state);
                movement_prediction.offset = Vec3::ZERO;
                // The first snapshot initializes position and orientation without interpolation.
                if was_uninitialized {
                    translation_interpolation.0.reset(state.translation);
                    if let Some(camera) = controller.camera
                        && let Ok(mut transform) = cameras.get_mut(camera)
                    {
                        transform.translation = Vec3::from_array(state.translation);
                        transform.rotation = bevy::prelude::Quat::from_array(state.rotation);
                    }
                    continue;
                }
                // Detached cameras retain their local pose while the player baseline advances.
                if controller.spirit_walking {
                    translation_interpolation.0.reset(state.translation);
                    continue;
                }
                let translation_p0 = controller
                    .camera
                    .and_then(|camera| cameras.get_mut(camera).ok())
                    .map(|transform| transform.translation.to_array())
                    .unwrap_or_else(|| translation_interpolation.0.value());
                translation_interpolation
                    .0
                    .retarget(translation_p0, state.translation);
            }
            ClientMarionetteCommand::ClearPlayerState => {
                authoritative.0 = None;
                controller.spirit_walking = false;
                translation_interpolation.0.reset([0.0; 3]);
                movement_prediction.offset = Vec3::ZERO;
                *controller_commands = ClientControllerCommandState::default();
                rotation_sync.reset();
            }
        }
    }
}

// Advances visual translation only while the camera follows the player.
fn interpolate_player_translation(
    time: Res<Time>,
    authoritative: Res<AuthoritativePlayerState>,
    controller: Res<ClientPlayerController>,
    mut translation_interpolation: ResMut<PlayerTranslationInterpolation>,
    movement_prediction: Res<ClientMovementPredictionState>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if authoritative.0.is_none() || controller.spirit_walking {
        return;
    }
    let translation = Vec3::from_array(translation_interpolation.0.advance(time.delta_secs()));
    if let Some(camera) = controller.camera
        && let Ok(mut transform) = cameras.get_mut(camera)
    {
        transform.translation = translation + movement_prediction.offset;
    }
}

// F1 detaches or restores the camera against the latest authoritative pose.
fn toggle_spirit_walking(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<ClientInputBindings>,
    authoritative: Res<AuthoritativePlayerState>,
    mut controller: ResMut<ClientPlayerController>,
    mut translation_interpolation: ResMut<PlayerTranslationInterpolation>,
    mut movement_prediction: ResMut<ClientMovementPredictionState>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if !controller.input_enabled
        || !bindings.just_pressed(SPIRIT_CAMERA_INPUT_SLOT, &keyboard, &mouse)
    {
        return;
    }
    let (Some(state), Some(camera)) = (authoritative.0, controller.camera) else {
        return;
    };

    controller.spirit_walking = !controller.spirit_walking;
    translation_interpolation.0.reset(state.translation);
    movement_prediction.offset = Vec3::ZERO;
    // Switching authority modes invalidates pending local rotation uploads.
    rotation_sync.reset();
    if !controller.spirit_walking
        && let Ok(mut transform) = cameras.get_mut(camera)
    {
        transform.translation = Vec3::from_array(state.translation);
        transform.rotation = bevy::prelude::Quat::from_array(state.rotation);
    }
}

#[cfg(test)]
mod tests;
