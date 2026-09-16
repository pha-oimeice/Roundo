//! Immediate local camera rotation with rate-limited server synchronization.

use super::{ROTATION_SYNC_INTERVAL_SECS, RotationSync};
use crate::character_controller::client::{
    AuthoritativePlayerState, ClientMarionetteEvent, ClientPipeResource, ClientPlayerController,
};
use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::{Camera, EulerRot, Query, Res, ResMut, Resource, Time, Transform, With},
};
use roundo_contracts::PlayerControllerCommand;

#[derive(Resource, Clone, Copy, Debug)]
/// Tracks whether a changed camera rotation still needs transmission.
pub(crate) struct ClientRotationSyncState {
    elapsed_secs: f32,
    pending: bool,
}

impl Default for ClientRotationSyncState {
    fn default() -> Self {
        Self {
            elapsed_secs: ROTATION_SYNC_INTERVAL_SECS,
            pending: false,
        }
    }
}

impl ClientRotationSyncState {
    /// Restores an immediately eligible, non-pending synchronization state.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
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
    // Spirit rotation is local-only and must not alter the controlled player.
    if !controller.spirit_walking {
        rotation_sync.pending = true;
    }
}

/// Sends the latest pending rotation at the configured maximum frequency.
pub(crate) fn synchronize_rotation(
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    cameras: Query<&Transform, With<Camera>>,
) {
    if !controller.input_enabled || controller.spirit_walking || authoritative.0.is_none() {
        return;
    }
    rotation_sync.elapsed_secs += time.delta_secs();
    if !rotation_sync.pending || rotation_sync.elapsed_secs < ROTATION_SYNC_INTERVAL_SECS {
        return;
    }
    let Some(camera) = controller.camera else {
        return;
    };
    let Ok(transform) = cameras.get(camera) else {
        return;
    };
    let sent = pipe
        .0
        .try_send(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::SyncRotation(RotationSync {
                rotation: transform.rotation.to_array(),
            }),
        ))
        .is_ok();
    // Failed delivery remains pending for the next interval.
    rotation_sync.elapsed_secs = 0.0;
    if sent {
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
