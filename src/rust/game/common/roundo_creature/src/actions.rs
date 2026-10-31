//! Execution-local work projected from typed Commands, not Body attributes.

use crate::CreatureMotionError;
use bevy::prelude::{Component, Vec3};

/// Execution-local action projected from a typed Command. It contains no
/// Controller identity, sequencing state, or mutable access to an Intent Body.
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct MovementAction {
    pub local_axis: [f32; 3],
}
impl MovementAction {
    pub fn new(local_axis: [f32; 3]) -> Result<Self, CreatureMotionError> {
        validate_axis(local_axis)?;
        Ok(Self { local_axis })
    }
}
#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct GazeAction {
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}
impl GazeAction {
    pub fn new(yaw_delta: f32, pitch_delta: f32) -> Result<Self, CreatureMotionError> {
        validate_gaze(yaw_delta, pitch_delta)?;
        Ok(Self {
            yaw_delta,
            pitch_delta,
        })
    }
}
fn validate_axis(local_axis: [f32; 3]) -> Result<(), CreatureMotionError> {
    let axis = Vec3::from_array(local_axis);
    if !axis.is_finite() || axis.length_squared() > 1.0 + f32::EPSILON {
        Err(CreatureMotionError::InvalidValue)
    } else {
        Ok(())
    }
}

fn validate_gaze(yaw_delta: f32, pitch_delta: f32) -> Result<(), CreatureMotionError> {
    if !yaw_delta.is_finite() || !pitch_delta.is_finite() {
        Err(CreatureMotionError::InvalidValue)
    } else {
        Ok(())
    }
}
