//! Immediate local camera rotation with rate-limited server synchronization.

use super::ROTATION_SYNC_INTERVAL_SECS;
use crate::character_controller::client::{
    ClientMarionetteEvent, ClientPipeResource, ClientPlayerController,
    ClientPlayerControllerAccess, LocallyRoutedControllerIntent,
};
use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::{
        Camera, EulerRot, MessageWriter, Query, Res, ResMut, Resource, Time, Transform, With,
    },
};
use roundo_contracts::PlayerControllerCommand;

#[derive(Resource, Clone, Copy, Debug)]
/// Tracks whether a changed camera rotation still needs transmission.
pub(crate) struct ClientRotationSyncState {
    elapsed_secs: f32,
    pending: bool,
    next_sequence: u64,
    yaw_delta: f32,
    pitch_delta: f32,
}

impl Default for ClientRotationSyncState {
    fn default() -> Self {
        Self {
            elapsed_secs: ROTATION_SYNC_INTERVAL_SECS,
            pending: false,
            next_sequence: 0,
            yaw_delta: 0.0,
            pitch_delta: 0.0,
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
        self.yaw_delta = 0.0;
        self.pitch_delta = 0.0;
    }
}

/// Applies accumulated mouse motion and marks authoritative rotation dirty.
pub(crate) fn rotate_camera(
    mouse_motion: Res<AccumulatedMouseMotion>,
    settings: Res<crate::character_controller::client::ClientMarionetteInputSettings>,
    controller: Res<ClientPlayerController>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut cameras: Query<&mut Transform, With<Camera>>,
) {
    if !controller.input_enabled {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };
    let Ok(mut transform) = cameras.get_mut(camera) else {
        return;
    };
    let yaw_delta = -mouse_motion.delta.x * settings.mouse_sensitivity;
    let pitch_delta = -mouse_motion.delta.y * settings.mouse_sensitivity;
    if !apply_camera_rotation(&mut transform, yaw_delta, pitch_delta) {
        return;
    }
    // Detached-camera rotation is local-only: it must not enter the controlled
    // Creature's gaze accumulator or be replayed after binding again.
    if !controller.spirit_walking {
        rotation_sync.yaw_delta += yaw_delta;
        rotation_sync.pitch_delta += pitch_delta;
        rotation_sync.pending = true;
    }
}

/// Sends the latest pending rotation at the configured maximum frequency.
pub(crate) fn synchronize_rotation(
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    access: Option<Res<ClientPlayerControllerAccess>>,
    pipe: Res<ClientPipeResource>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut routed_intents: MessageWriter<LocallyRoutedControllerIntent>,
    cameras: Query<&Transform, With<Camera>>,
) {
    if !controller.input_enabled || controller.spirit_walking {
        return;
    }
    let controller_id = match access {
        Some(access) => match access
            .gaze_controller()
            .filter(|id| access.is_controlled_by_self(*id))
        {
            Some(id) => Some(id),
            None => return,
        },
        None => None,
    };
    rotation_sync.elapsed_secs += time.delta_secs();
    if !rotation_sync.pending || rotation_sync.elapsed_secs < ROTATION_SYNC_INTERVAL_SECS {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };
    let Ok(_transform) = cameras.get(camera) else {
        return;
    };
    let routed = PlayerControllerCommand::Gaze(roundo_contracts::ControllerCommand {
        sequence: rotation_sync.next_sequence.wrapping_add(1),
        action: roundo_contracts::GazeIntent {
            yaw_delta: rotation_sync.yaw_delta,
            pitch_delta: rotation_sync.pitch_delta,
        },
    });
    let sent = pipe
        .0
        .try_send(match controller_id {
            Some(controller_id) => ClientMarionetteEvent::SubmitControllerInput {
                controller_id,
                input: roundo_contracts::DirectedControllerInput::Gaze(
                    roundo_contracts::ControllerCommand {
                        sequence: rotation_sync.next_sequence.wrapping_add(1),
                        action: roundo_contracts::GazeIntent {
                            yaw_delta: rotation_sync.yaw_delta,
                            pitch_delta: rotation_sync.pitch_delta,
                        },
                    },
                ),
            },
            None => ClientMarionetteEvent::UsePlayerController(routed),
        })
        .is_ok();
    // Failed delivery remains pending for the next interval.
    rotation_sync.elapsed_secs = 0.0;
    if sent {
        rotation_sync.next_sequence = rotation_sync.next_sequence.wrapping_add(1);
        routed_intents.write(LocallyRoutedControllerIntent(routed));
        rotation_sync.yaw_delta = 0.0;
        rotation_sync.pitch_delta = 0.0;
        rotation_sync.pending = false;
    } else {
        log::warn!("cannot synchronize player rotation: reason=client_bridge_closed");
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
