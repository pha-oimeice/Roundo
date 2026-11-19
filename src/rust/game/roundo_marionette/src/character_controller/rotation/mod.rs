//! Local gaze intent validation and camera rotation.

mod client;
use bevy::prelude::{Component, Entity, Message};
pub use roundo_contracts::GazeIntent;

pub(super) use self::client::rotate_camera;

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
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct AcceptedGazeIntent {
    pub controller: Entity,
    pub sequence: u64,
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct GazeIntentMessage {
    pub entity: Entity,
    pub command: roundo_contracts::ControllerCommand<GazeIntent>,
}
