//! Native spatial and environment attribute values owned by Local Coordinate.
//!
//! These definitions are separate from transform propagation, spatial indexes
//! and environment storage. No generic value container or new coordinate copy is
//! introduced. A local-coordinate world is not implicitly a Concept Body.

use bevy::prelude::{Component, Quat, Transform, Vec3};

/// Authoritative spatial attribute of a local-coordinate entity. Engine
/// Transform is derived from this value before hierarchy propagation.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(Transform)]
pub struct LocalCoordinateTransform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl LocalCoordinateTransform {
    /// Identity transform used by static generated coordinates.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// Constructs an unrotated, unit-scale transform.
    pub const fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// Constructs a transform from explicit translation, rotation, and scale.
    pub const fn from_parts(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// Replaces scale while preserving translation and rotation.
    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    pub(crate) fn as_bevy(self) -> Transform {
        Transform {
            translation: self.translation,
            rotation: self.rotation,
            scale: self.scale,
        }
    }
}

impl Default for LocalCoordinateTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Complete environment values used when a virtual cell has no sparse override.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompleteEnvironmentDefaults {
    pub gravity: Vec3,
    pub static_friction: f32,
    pub kinetic_friction: f32,
}

impl Default for CompleteEnvironmentDefaults {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            static_friction: 0.6,
            kinetic_friction: 0.5,
        }
    }
}

/// Changed environment fields for one virtual cell. `None` means use default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SparseEnvironmentOverride {
    pub gravity: Option<Vec3>,
    pub static_friction: Option<f32>,
    pub kinetic_friction: Option<f32>,
}

pub(super) fn validate_environment(
    gravity: Vec3,
    static_friction: f32,
    kinetic_friction: f32,
) -> bool {
    gravity.is_finite()
        && static_friction.is_finite()
        && kinetic_friction.is_finite()
        && static_friction >= 0.0
        && kinetic_friction >= 0.0
        && kinetic_friction <= static_friction
}
