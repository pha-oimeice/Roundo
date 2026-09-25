//! Client controller scheduling and authoritative-update routing.

use super::*;
#[cfg(feature = "dev")]
use crate::character_controller::test_creature::route_spawn_test_creature;
use crate::character_controller::{
    ClientInputBindings, SPIRIT_CAMERA_INPUT_SLOT,
    block_interaction::route_block_interactions,
    movement::{move_spirit_camera, route_player_movement},
    rotation::{ClientRotationSyncState, rotate_camera, synchronize_rotation},
};
use bevy::prelude::{
    ButtonInput, Camera, IntoScheduleConfigs, KeyCode, MessageWriter, MouseButton, Query, Res,
    ResMut, Time, Transform, Update, Vec3, With,
};

// Ordering is intentional: authority updates precede local input and upload.
impl Plugin for MarionetteClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientPlayerController>()
            .init_resource::<ClientPlayerControllerAccess>()
            .init_resource::<ClientMarionetteInputSettings>()
            .init_resource::<ClientInputBindings>()
            .init_resource::<ClientMarionetteSession>()
            .init_resource::<AuthoritativePlayerState>()
            .init_resource::<PlayerTranslationInterpolation>()
            .init_resource::<ClientControllerCommandState>()
            .init_resource::<ClientRotationSyncState>()
            .insert_resource(ClientPipeResource(self.pipe.endpoint_b()))
            .add_message::<CreatureAuthorityUpdate>()
            .add_message::<LocallyRoutedControllerIntent>()
            .configure_sets(
                Update,
                (MarionetteClientSet::Commands, MarionetteClientSet::Input).chain(),
            )
            .add_systems(
                Update,
                process_server_commands.in_set(MarionetteClientSet::Commands),
            )
            .add_systems(
                Update,
                (
                    toggle_spirit_walking,
                    rotate_camera,
                    move_spirit_camera,
                    route_player_movement,
                    route_block_interactions,
                    synchronize_rotation,
                )
                    .chain()
                    .in_set(MarionetteClientSet::Input),
            );
        #[cfg(feature = "dev")]
        app.add_systems(
            Update,
            route_spawn_test_creature
                .after(route_block_interactions)
                .in_set(MarionetteClientSet::Input),
        );
    }
}

// Routes authority changes; Creature Prediction owns baseline and replay state.
fn process_server_commands(
    pipe: Res<ClientPipeResource>,
    mut session: ResMut<ClientMarionetteSession>,
    mut authoritative: ResMut<AuthoritativePlayerState>,
    mut creature_authority: MessageWriter<CreatureAuthorityUpdate>,
    mut controller: ResMut<ClientPlayerController>,
    mut translation_interpolation: ResMut<PlayerTranslationInterpolation>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut access: Option<ResMut<ClientPlayerControllerAccess>>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientMarionetteCommand::RequestPlayerControllerAccess if session.0.is_some() => {
                let _ = pipe
                    .0
                    .try_send(ClientMarionetteEvent::RequestPlayerControllerAccess);
            }
            ClientMarionetteCommand::AcquireController { controller_id } if session.0.is_some() => {
                let _ = pipe
                    .0
                    .try_send(ClientMarionetteEvent::AcquireController { controller_id });
            }
            ClientMarionetteCommand::ReleaseController { controller_id } if session.0.is_some() => {
                let _ = pipe
                    .0
                    .try_send(ClientMarionetteEvent::ReleaseController { controller_id });
            }
            ClientMarionetteCommand::RequestPlayerControllerAccess
            | ClientMarionetteCommand::AcquireController { .. }
            | ClientMarionetteCommand::ReleaseController { .. } => {}
            ClientMarionetteCommand::BeginSession { epoch } => {
                reset_controller_session(
                    &mut authoritative,
                    &mut controller,
                    &mut translation_interpolation,
                    &mut controller_commands,
                    &mut rotation_sync,
                );
                if let Some(access) = &mut access {
                    access.clear();
                }
                session.0 = Some(epoch);
                let _ = pipe
                    .0
                    .try_send(ClientMarionetteEvent::RequestPlayerControllerAccess);
                creature_authority.write(CreatureAuthorityUpdate::SessionStarted { epoch });
            }
            ClientMarionetteCommand::PlayerControllerAccessSnapshot { epoch, snapshot }
                if session.0 == Some(epoch) =>
            {
                let Some(access) = &mut access else {
                    continue;
                };
                access.movement = snapshot.controllers.iter().find_map(|entry| {
                    (entry.intent_domain == roundo_contracts::ControllerIntentDomain::Movement)
                        .then_some(entry.controller_id)
                });
                access.gaze = snapshot.controllers.iter().find_map(|entry| {
                    (entry.intent_domain == roundo_contracts::ControllerIntentDomain::Gaze)
                        .then_some(entry.controller_id)
                });
                access.snapshot = Some(snapshot);
                access.last_rejection = None;
            }
            ClientMarionetteCommand::ControllerAcquired {
                epoch,
                controller_id,
            } if session.0 == Some(epoch) => {
                if let Some(access) = &mut access
                    && let Some(snapshot) = access.snapshot.as_mut()
                {
                    if let Some(entry) = snapshot
                        .controllers
                        .iter_mut()
                        .find(|entry| entry.controller_id == controller_id)
                    {
                        entry.control_state =
                            roundo_contracts::ControllerControlState::ControlledBySelf;
                    }
                }
            }
            ClientMarionetteCommand::ControllerReleased {
                epoch,
                controller_id,
            } if session.0 == Some(epoch) => {
                if let Some(access) = &mut access
                    && let Some(snapshot) = access.snapshot.as_mut()
                {
                    if let Some(entry) = snapshot
                        .controllers
                        .iter_mut()
                        .find(|entry| entry.controller_id == controller_id)
                    {
                        entry.control_state =
                            roundo_contracts::ControllerControlState::Uncontrolled;
                    }
                }
            }
            ClientMarionetteCommand::ControllerOperationRejected {
                epoch,
                operation,
                error,
            } if session.0 == Some(epoch) => {
                if let Some(access) = &mut access {
                    access.last_rejection = Some((operation, error));
                }
            }
            ClientMarionetteCommand::PlayerControllerAccessSnapshot { .. }
            | ClientMarionetteCommand::ControllerAcquired { .. }
            | ClientMarionetteCommand::ControllerReleased { .. }
            | ClientMarionetteCommand::ControllerOperationRejected { .. } => {}
            ClientMarionetteCommand::CreatureMotionSnapshot(snapshot) => {
                if let Some(epoch) = session.0 {
                    creature_authority.write(CreatureAuthorityUpdate::Snapshot { epoch, snapshot });
                }
            }
            ClientMarionetteCommand::PlayerState(state) if session.0.is_some() => {
                let was_uninitialized = authoritative.0.is_none();
                authoritative.0 = Some(state);
                // Presence remains a remote/interpolated projection. The controlled
                // Creature and its camera reconcile only from Creature snapshots.
                if was_uninitialized || controller.spirit_walking {
                    translation_interpolation.0.reset(state.translation);
                }
            }
            ClientMarionetteCommand::PlayerState(_) => {}
            ClientMarionetteCommand::EndSession { epoch } if session.0 == Some(epoch) => {
                reset_controller_session(
                    &mut authoritative,
                    &mut controller,
                    &mut translation_interpolation,
                    &mut controller_commands,
                    &mut rotation_sync,
                );
                session.0 = None;
                if let Some(access) = &mut access {
                    access.clear();
                }
                creature_authority.write(CreatureAuthorityUpdate::SessionEnded { epoch });
            }
            ClientMarionetteCommand::EndSession { .. } => {}
        }
    }
}

fn reset_controller_session(
    authoritative: &mut AuthoritativePlayerState,
    controller: &mut ClientPlayerController,
    translation_interpolation: &mut PlayerTranslationInterpolation,
    controller_commands: &mut ClientControllerCommandState,
    rotation_sync: &mut ClientRotationSyncState,
) {
    authoritative.0 = None;
    controller.spirit_walking = true;
    translation_interpolation.0.reset([0.0; 3]);
    *controller_commands = ClientControllerCommandState::default();
    rotation_sync.reset();
}

// Legacy remote-player interpolation helper retained for isolated interpolation tests.
#[cfg_attr(not(test), allow(dead_code))]
fn interpolate_player_translation(
    time: Res<Time>,
    authoritative: Res<AuthoritativePlayerState>,
    controller: Res<ClientPlayerController>,
    mut translation_interpolation: ResMut<PlayerTranslationInterpolation>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if authoritative.0.is_none() || controller.spirit_walking {
        return;
    }
    let translation = Vec3::from_array(translation_interpolation.0.advance(time.delta_secs()));
    if let Some(camera) = controller.camera
        && let Ok(mut transform) = cameras.get_mut(camera)
    {
        transform.translation = translation;
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
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if !controller.input_enabled
        || !bindings.just_pressed(SPIRIT_CAMERA_INPUT_SLOT, &keyboard, &mouse)
    {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };

    controller.spirit_walking = !controller.spirit_walking;
    // Switching projection modes invalidates unsent mouse deltas, but this is
    // still the same Controller session, so gaze command sequencing must survive.
    rotation_sync.discard_pending();
    let Some(state) = authoritative.0 else {
        // Creature Prediction owns the controlled pose and will bind the camera
        // on its next update; F1 remains only a projection-mode toggle.
        return;
    };
    translation_interpolation.0.reset(state.translation);
    if !controller.spirit_walking
        && let Ok(mut transform) = cameras.get_mut(camera)
    {
        transform.translation = Vec3::from_array(state.translation);
        transform.rotation = bevy::prelude::Quat::from_array(state.rotation);
    }
}

#[cfg(test)]
mod tests;
