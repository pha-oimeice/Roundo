//! Absolute look selection. The Intent origin writes; behavior readers only copy.
use crate::CreatureMotionError;
use bevy::prelude::Quat;

/// Held absolute gaze selection. None retains the recipe's spawn/legacy baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GazeCommandState {
    orientation: Option<[f32; 4]>,
}
impl GazeCommandState {
    pub fn new(orientation: [f32; 4]) -> Result<Self, CreatureMotionError> {
        let rotation = Quat::from_array(orientation);
        if !rotation.is_finite() || !rotation.is_normalized() {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            orientation: Some(orientation),
        })
    }
    pub const fn orientation(&self) -> Option<[f32; 4]> {
        self.orientation
    }
}
roundo_life::declare_state_data!(GazeCommandState, "roundo.creature.gaze-command-state.v1");
