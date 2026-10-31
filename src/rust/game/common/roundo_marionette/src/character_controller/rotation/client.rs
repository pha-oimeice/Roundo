//! Immediate local look selection with rate-limited absolute server synchronization.

use super::ROTATION_SYNC_INTERVAL_SECS;
use crate::character_controller::client::{
    ClientMarionetteEvent, ClientPipeResource, ClientPlayerController,
    ClientPlayerControllerAccess, LocallyRoutedControllerIntent,
};
use crate::character_controller::{ClientInputFrame, LOOK_INPUT_SLOT};
use bevy::prelude::{
    Camera, EulerRot, MessageReader, MessageWriter, Query, Res, ResMut, Resource, Time, Transform,
    With,
};
use roundo_contracts::PlayerControllerCommand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RotationUploadMode {
    RateLimited,
    Flush,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RotationUploadOutcome {
    Submitted,
    NotEligible,
    Idle,
    Throttled,
    CameraUnavailable,
    SequenceExhausted,
    BridgeClosed,
}

#[derive(Resource, Clone, Copy, Debug)]
/// Tracks whether a changed camera rotation still needs transmission.
pub(crate) struct ClientRotationSyncState {
    elapsed_secs: f32,
    pending: bool,
    next_sequence: u64,
}

impl Default for ClientRotationSyncState {
    fn default() -> Self {
        Self {
            elapsed_secs: ROTATION_SYNC_INTERVAL_SECS,
            pending: true,
            next_sequence: 0,
        }
    }
}

impl ClientRotationSyncState {
    /// Clears all connection-scoped gaze sequencing and pending state.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Drops only unsent camera motion while preserving the controller's
    /// monotonic sequence across detach/rebind transitions.
    pub(crate) fn discard_pending(&mut self) {
        self.elapsed_secs = ROTATION_SYNC_INTERVAL_SECS;
        self.pending = false;
    }
}

/// Applies accumulated mouse motion and marks authoritative rotation dirty.
pub(crate) fn rotate_camera(
    mut inputs: MessageReader<ClientInputFrame>,
    settings: Res<crate::character_controller::client::ClientMarionetteInputSettings>,
    controller: Res<ClientPlayerController>,
    access: Option<Res<ClientPlayerControllerAccess>>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
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
    let delta = input
        .motion
        .get(LOOK_INPUT_SLOT)
        .copied()
        .unwrap_or_default();
    let yaw_delta = -delta.x * settings.mouse_sensitivity;
    let pitch_delta = -delta.y * settings.mouse_sensitivity;
    if !apply_camera_rotation(&mut transform, yaw_delta, pitch_delta) {
        return;
    }
    // Detached-camera rotation is local-only: it must not enter the controlled
    // Creature's gaze accumulator or be replayed after binding again.
    if !controller.spirit_walking
        && !access
            .as_ref()
            .is_some_and(|access| access.blocks_pending_gaze())
    {
        rotation_sync.pending = true;
    }
}

/// Uploads the current absolute rotation, preserving pending and sequence on failure.
pub(crate) fn synchronize_current_rotation(
    time: &Time,
    controller: &ClientPlayerController,
    access: &ClientPlayerControllerAccess,
    pipe: &ClientPipeResource,
    rotation_sync: &mut ClientRotationSyncState,
    mode: RotationUploadMode,
    routed_intents: &mut MessageWriter<LocallyRoutedControllerIntent>,
    cameras: &Query<&Transform, With<Camera>>,
) -> RotationUploadOutcome {
    if !controller.input_enabled || controller.spirit_walking {
        return RotationUploadOutcome::NotEligible;
    }
    let Some(controller_id) =
        access.producing_controller(roundo_contracts::ControllerIntentDomain::Gaze)
    else {
        return RotationUploadOutcome::NotEligible;
    };
    rotation_sync.elapsed_secs += time.delta_secs();
    if mode == RotationUploadMode::RateLimited {
        if !rotation_sync.pending {
            return RotationUploadOutcome::Idle;
        }
        if rotation_sync.elapsed_secs < ROTATION_SYNC_INTERVAL_SECS {
            return RotationUploadOutcome::Throttled;
        }
    }
    let Some(camera) = controller.camera else {
        return RotationUploadOutcome::CameraUnavailable;
    };
    let Ok(transform) = cameras.get(camera) else {
        return RotationUploadOutcome::CameraUnavailable;
    };
    let Some(sequence) = rotation_sync.next_sequence.checked_add(1) else {
        return RotationUploadOutcome::SequenceExhausted;
    };
    let command = roundo_contracts::ControllerCommand {
        sequence,
        action: roundo_contracts::SetGazeIntent {
            orientation: transform.rotation.to_array(),
        },
    };
    let routed = PlayerControllerCommand::SetGaze(command);
    let sent = pipe
        .0
        .try_send(ClientMarionetteEvent::SubmitControllerInput {
            controller_id,
            input: roundo_contracts::DirectedControllerInput::SetGaze(command),
        })
        .is_ok();
    // Only an actual send attempt resets the elapsed interval.
    rotation_sync.elapsed_secs = 0.0;
    if sent {
        rotation_sync.next_sequence = sequence;
        routed_intents.write(LocallyRoutedControllerIntent(routed));
        rotation_sync.pending = false;
        RotationUploadOutcome::Submitted
    } else {
        log::warn!("cannot synchronize player rotation: reason=client_bridge_closed");
        RotationUploadOutcome::BridgeClosed
    }
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
