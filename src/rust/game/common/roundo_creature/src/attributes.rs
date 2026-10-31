//! Native attributes owned by Creature's motion module.
//!
//! Definitions and value invariants live here; systems still query the concrete
//! components directly. This is not a dynamic property bag or an authorization
//! interface. Actions, sampled environment, prototypes and runtime assembly have
//! separate owners. A Body need not contain every attribute in this module.

use crate::CreatureMotionError;
use bevy::prelude::{Component, Quat, Vec3};

/// Native spatial value used by the Execution solver. Its translation is the
/// logical bottom-center anchor, not the collider center. This is Bevy's existing
/// component, not another pose store; Record/presentation copies are projections.
pub use bevy::prelude::Transform;

/// Native physics components used by the existing Execution recipe. They retain
/// Avian's storage, hooks and synchronization; no parallel property value exists.
pub use avian3d::prelude::{Collider, RigidBody};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LocomotionState {
    Grounded,
    #[default]
    Floating,
}
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct KinematicMass(pub f32);

impl KinematicMass {
    pub fn new(mass: f32) -> Result<Self, CreatureMotionError> {
        if !mass.is_finite() || mass <= 0.0 {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self(mass))
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Default)]
pub struct KinematicVelocity(pub Vec3);

/// Tunable response owned by an Execution Body with grounded locomotion.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GroundLocomotion {
    pub(crate) max_speed: f32,
    pub(crate) acceleration: f32,
    pub(crate) braking_acceleration: f32,
    pub(crate) jump_speed: f32,
}

impl GroundLocomotion {
    /// Conservative displacement bound before collision, including existing
    /// momentum, gravity, horizontal control and a possible ground jump.
    /// Requires finite vectors and a finite nonnegative timestep. A non-finite
    /// result supplies no usable bound; readiness callers must close their gate.
    pub fn sweep_distance_bound(self, velocity: Vec3, gravity: Vec3, dt: f32) -> f32 {
        (velocity.length() + gravity.length() * dt + self.max_speed + self.jump_speed) * dt
    }
    pub fn new(
        max_speed: f32,
        acceleration: f32,
        braking_acceleration: f32,
        jump_speed: f32,
    ) -> Result<Self, CreatureMotionError> {
        if !max_speed.is_finite()
            || max_speed < 0.0
            || !acceleration.is_finite()
            || acceleration < 0.0
            || !braking_acceleration.is_finite()
            || braking_acceleration < 0.0
            || !jump_speed.is_finite()
            || jump_speed < 0.0
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            max_speed,
            acceleration,
            braking_acceleration,
            jump_speed,
        })
    }
}

/// Movement geometry shared by every Creature Execution and prediction.
/// Axes are world-aligned, independent of body/gaze orientation and gravity.
/// The logical origin is the center of the minimum-Y face; dimensions, not
/// Transform scale, own the physical size. Change this component to resize.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct AabbDimensions {
    half_extents: Vec3,
}

impl AabbDimensions {
    pub fn new(half_extents: Vec3) -> Result<Self, CreatureMotionError> {
        if !half_extents.is_finite()
            || !half_extents.cmpgt(Vec3::ZERO).all()
            || !(half_extents * 2.0).is_finite()
        {
            Err(CreatureMotionError::InvalidValue)
        } else {
            Ok(Self { half_extents })
        }
    }

    pub const fn half_extents(self) -> Vec3 {
        self.half_extents
    }

    pub fn size(self) -> Vec3 {
        self.half_extents * 2.0
    }

    /// World-axis offset from the logical bottom center to the physical center.
    pub fn center_offset(self) -> Vec3 {
        Vec3::Y * self.half_extents.y
    }

    pub fn center_at(self, logical_position: Vec3) -> Vec3 {
        logical_position + self.center_offset()
    }

    /// Avian shape in the logical frame. Its transform must remain unrotated
    /// and unscaled so both spatial queries and physics use the same AABB.
    pub(crate) fn collider(self) -> Collider {
        let size = self.size();
        Collider::compound(vec![(
            self.center_offset(),
            Quat::IDENTITY,
            Collider::cuboid(size.x, size.y, size.z),
        )])
    }
}

/// Maximum resting separation and the base tolerance used by ground snapping.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ContactTolerance(pub f32);

impl ContactTolerance {
    pub fn new(tolerance: f32) -> Result<Self, CreatureMotionError> {
        if !tolerance.is_finite() || tolerance < 0.0 {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self(tolerance))
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct BodyOrientation(pub Quat);
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GazeOrientation(pub Quat);
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GroundHorizontalMovement;
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GroundJump;
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GazeControl;
