//! Immutable environment sampled for a solve, not a Body's intrinsic settings.
//!
//! The authoritative environment provider remains external to Creature; retaining
//! this sample does not create a second writable environment authority.

use crate::CreatureMotionError;
use bevy::prelude::{Component, Vec3};

/// Complete, immutable local environment for one logic tick.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentSample {
    pub gravity: Vec3,
    pub static_friction: f32,
    pub kinetic_friction: f32,
    pub revision: u64,
}
impl EnvironmentSample {
    pub fn new(
        gravity: Vec3,
        static_friction: f32,
        kinetic_friction: f32,
        revision: u64,
    ) -> Result<Self, CreatureMotionError> {
        if !gravity.is_finite()
            || !static_friction.is_finite()
            || !kinetic_friction.is_finite()
            || static_friction < 0.0
            || kinetic_friction < 0.0
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        if kinetic_friction > static_friction {
            return Err(CreatureMotionError::InvalidFriction);
        }
        Ok(Self {
            gravity,
            static_friction,
            kinetic_friction,
            revision,
        })
    }
}
impl Default for EnvironmentSample {
    fn default() -> Self {
        Self::new(Vec3::new(0.0, -9.81, 0.0), 0.6, 0.5, 0).unwrap()
    }
}
