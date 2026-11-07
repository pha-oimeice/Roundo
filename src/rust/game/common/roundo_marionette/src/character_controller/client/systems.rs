//! Client controller scheduling and authoritative-update routing.

use super::*;
#[cfg(feature = "dev")]
use crate::character_controller::test_creature::route_spawn_test_creature;
use crate::character_controller::{
    ClientFunctionFrame, SPIRIT_CAMERA_FUNCTION,
    movement::{move_spirit_camera, route_player_movement},
    rotation::{ClientRotationSyncState, rotate_camera, route_current_gaze},
};
use bevy::prelude::{
    Camera, IntoScheduleConfigs, MessageReader, MessageWriter, Query, Res, ResMut, Transform,
    Update, With,
};

// Ordering is intentional: authority updates precede local input and upload.
impl Plugin for MarionetteClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientPlayerController>()
            .init_resource::<ClientPlayerControllerAccess>()
            .init_resource::<ClientMarionetteInputSettings>()
            .add_message::<ClientFunctionFrame>()
            .init_resource::<AuthoritativePlayerState>()
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
                    route_current_gaze,
                )
                    .chain()
                    .in_set(MarionetteClientSet::Input),
            );
        #[cfg(feature = "dev")]
        app.add_systems(
            Update,
            route_spawn_test_creature
                .after(route_current_gaze)
                .in_set(MarionetteClientSet::Input),
        );
    }
}

// Routes authority changes; Creature Prediction owns baseline and replay state.
fn process_server_commands(
    pipe: Res<ClientPipeResource>,
    mut authoritative: ResMut<AuthoritativePlayerState>,
    mut creature_authority: MessageWriter<CreatureAuthorityUpdate>,
    mut controller: ResMut<ClientPlayerController>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut access: ResMut<ClientPlayerControllerAccess>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientMarionetteCommand::Freeze { epoch, freeze } => {
                if let Some(barrier) = access.apply_freeze(epoch, freeze, &mut rotation_sync) {
                    // Failed delivery cannot report a successful drain or reopen input.
                    if pipe
                        .0
                        .try_send(ClientMarionetteEvent::DrainBarrier(barrier))
                        .is_err()
                    {
                        log::warn!(
                            "cannot enqueue controller drain barrier; producer stays frozen"
                        );
                    }
                }
            }
            ClientMarionetteCommand::ControlPreparing { epoch, preparing } => {
                if let Some(update) = access.apply_preparing(
                    epoch,
                    preparing,
                    &mut controller_commands,
                    &mut rotation_sync,
                ) {
                    creature_authority.write(update);
                }
            }
            ClientMarionetteCommand::TicketAcquired {
                epoch,
                request_id,
                controller_id,
                binding,
            } => {
                if let Some(update) =
                    access.apply_ticket_acquired(epoch, request_id, controller_id, binding)
                {
                    creature_authority.write(update);
                }
            }
            ClientMarionetteCommand::TicketActivated {
                epoch,
                request_id,
                controller_id,
            } => {
                if access.apply_ticket_activated(epoch, request_id, controller_id) {
                    // Acquire the next domain only after this producer is activated.
                    let _ = pipe
                        .0
                        .try_send(ClientMarionetteEvent::RequestPlayerControllerAccess);
                }
            }
            ClientMarionetteCommand::TicketReleased {
                epoch,
                request_id,
                controller_id,
            } => {
                if let Some(update) = access.apply_ticket_released(
                    epoch,
                    request_id,
                    controller_id,
                    &mut controller_commands,
                    &mut rotation_sync,
                ) {
                    creature_authority.write(update);
                }
            }
            ClientMarionetteCommand::TicketCompleted {
                epoch,
                request_id,
                controller_id,
                outcome,
            } => {
                access.apply_ticket_completed(
                    epoch,
                    request_id,
                    controller_id,
                    outcome,
                    &mut controller_commands,
                    &mut rotation_sync,
                );
            }
            ClientMarionetteCommand::RequestPlayerControllerAccess => {
                if access.active_epoch().is_some() {
                    let _ = pipe
                        .0
                        .try_send(ClientMarionetteEvent::RequestPlayerControllerAccess);
                }
            }
            ClientMarionetteCommand::AcquireController { controller_id } => {
                if let Some(epoch) = access.active_epoch() {
                    let _ = pipe.0.try_send(ClientMarionetteEvent::AcquireController {
                        epoch,
                        controller_id,
                    });
                }
            }
            ClientMarionetteCommand::ReleaseController { controller_id } => {
                if access.active_epoch().is_some() {
                    let _ = pipe
                        .0
                        .try_send(ClientMarionetteEvent::ReleaseController { controller_id });
                }
            }
            ClientMarionetteCommand::BeginSession { epoch } => {
                let update =
                    access.begin_session(epoch, &mut controller_commands, &mut rotation_sync);
                authoritative.0 = None;
                controller.spirit_walking = false;
                creature_authority.write(update);
                // This marker follows all prior outbound work in the shared FIFO.
                // Failure leaves the network adapter closed for this epoch.
                if pipe
                    .0
                    .try_send(ClientMarionetteEvent::SessionActivated { epoch })
                    .is_ok()
                {
                    let _ = pipe
                        .0
                        .try_send(ClientMarionetteEvent::RequestPlayerControllerAccess);
                } else {
                    log::warn!("cannot activate Marionette outbound session: epoch={epoch}");
                }
            }
            ClientMarionetteCommand::PlayerControllerAccessSnapshot { epoch, snapshot } => {
                for controller_id in
                    access.apply_access_snapshot(epoch, snapshot, &mut rotation_sync)
                {
                    if pipe
                        .0
                        .try_send(ClientMarionetteEvent::AcquireController {
                            epoch,
                            controller_id,
                        })
                        .is_ok()
                    {
                        break;
                    }
                }
            }
            ClientMarionetteCommand::ControllerAcquired {
                epoch,
                controller_id,
            } => {
                access.apply_legacy_control_state(
                    epoch,
                    controller_id,
                    ControllerControlState::ControlledBySelf,
                );
            }
            ClientMarionetteCommand::ControllerReleased {
                epoch,
                controller_id,
            } => {
                access.apply_legacy_control_state(
                    epoch,
                    controller_id,
                    ControllerControlState::Uncontrolled,
                );
            }
            ClientMarionetteCommand::ControllerOperationRejected {
                epoch,
                operation,
                error,
            } => {
                access.apply_operation_rejected(epoch, operation, error);
            }
            ClientMarionetteCommand::CreatureMotionSnapshot(snapshot) => {
                if let Some(epoch) = access.active_epoch() {
                    creature_authority.write(CreatureAuthorityUpdate::Snapshot { epoch, snapshot });
                }
            }
            ClientMarionetteCommand::PlayerState(state) => {
                if access.active_epoch().is_some() {
                    authoritative.0 = Some(state);
                    // Presence remains a remote/interpolated projection.
                }
            }
            ClientMarionetteCommand::EndSession { epoch } => {
                if let Some(update) =
                    access.end_session(epoch, &mut controller_commands, &mut rotation_sync)
                {
                    authoritative.0 = None;
                    controller.spirit_walking = false;
                    creature_authority.write(update);
                }
            }
        }
    }
}

// The registered spirit-camera action changes only the local projection mode.
fn toggle_spirit_walking(
    mut inputs: MessageReader<ClientFunctionFrame>,
    authoritative: Res<AuthoritativePlayerState>,
    mut controller: ResMut<ClientPlayerController>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    let input = inputs.read().last().cloned().unwrap_or_default();
    if !controller.input_enabled || !input.just_pressed(SPIRIT_CAMERA_FUNCTION) {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };

    controller.spirit_walking = !controller.spirit_walking;
    // Switching projection modes invalidates an unsent look selection, but this is
    // still the same Controller session, so gaze command sequencing must survive.
    rotation_sync.discard_pending();
    let Some(state) = authoritative.0 else {
        // Creature Prediction owns the controlled pose and will bind the camera
        // on its next update; this semantic action only toggles projection mode.
        return;
    };
    if !controller.spirit_walking
        && let Ok(mut transform) = cameras.get_mut(camera)
    {
        // Translation belongs to the prediction camera projection, including its
        // AABB-center offset. Never temporarily place the camera at logical feet.
        transform.rotation = bevy::prelude::Quat::from_array(state.rotation);
    }
}
