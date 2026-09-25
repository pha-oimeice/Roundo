//! Controller-domain gaze intent validation and client upload.

mod client;
use bevy::prelude::{Component, Entity, Message};
pub use roundo_contracts::GazeIntent;

#[cfg(test)]
pub(super) use self::client::apply_camera_rotation;
pub(super) use self::client::{ClientRotationSyncState, rotate_camera, synchronize_rotation};

/// Minimum interval between client gaze intent uploads.
pub const ROTATION_SYNC_INTERVAL_SECS: f32 = 0.1;

/// Controller-owned sequence state. It owns neither pose nor orientation.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct GazeController {
    last_accepted_sequence: u64,
}
impl GazeController {
    pub(crate) fn accept(&mut self, sequence: u64, intent: GazeIntent) -> bool {
        if sequence <= self.last_accepted_sequence
            || !intent.yaw_delta.is_finite()
            || !intent.pitch_delta.is_finite()
        {
            return false;
        }
        self.last_accepted_sequence = sequence;
        true
    }
    pub(crate) fn last_accepted_sequence(self) -> u64 {
        self.last_accepted_sequence
    }
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct AcceptedGazeIntent {
    pub controller: Entity,
    pub sequence: u64,
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct GazeIntentMessage {
    pub entity: Entity,
    pub command: roundo_contracts::ControllerCommand<GazeIntent>,
}
