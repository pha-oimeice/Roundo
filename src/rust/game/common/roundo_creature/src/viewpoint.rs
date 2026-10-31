//! Camera-independent eye geometry from one coherent committed motion value.
use crate::{AabbDimensions, CreatureMotionError, CreatureMotionExperienceState};
use bevy::prelude::{Dir3, Quat, Ray3d, Vec3};

/// Finite AABB-center-relative eye offset and a positive ray range.
/// The logical pose is anchored at the bottom center; `ray` applies the physical
/// center offset before the eye offset. Both offsets use world-aligned axes,
/// independently of the locomotion frame, keeping the eye inside the AABB.
/// This is geometry, not a motion receipt, world readiness or edit permission.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatureViewpoint {
    eye_offset: Vec3,
    maximum_distance: f32,
    aabb: AabbDimensions,
}
impl CreatureViewpoint {
    pub fn new(
        eye_offset: Vec3,
        maximum_distance: f32,
        aabb: AabbDimensions,
    ) -> Result<Self, CreatureMotionError> {
        if !eye_offset.is_finite()
            || !maximum_distance.is_finite()
            || maximum_distance <= 0.0
            || !eye_offset.abs().cmple(aabb.half_extents()).all()
        {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Self {
            eye_offset,
            maximum_distance,
            aabb,
        })
    }
    pub const fn eye_offset(self) -> Vec3 {
        self.eye_offset
    }
    pub const fn maximum_distance(self) -> f32 {
        self.maximum_distance
    }
    pub const fn aabb(self) -> AabbDimensions {
        self.aabb
    }

    /// Uses the world-aligned AABB for the origin and gaze orientation for the ray.
    /// The caller must separately require an actual current-tick motion commit;
    /// an Experience-shaped value alone cannot certify that a solve happened.
    pub fn ray(self, motion: CreatureMotionExperienceState) -> Result<Ray3d, CreatureMotionError> {
        let origin = self.aabb.center_at(Vec3::from_array(*motion.translation())) + self.eye_offset;
        let direction = Quat::from_array(*motion.gaze_orientation()) * Vec3::NEG_Z;
        if !origin.is_finite() {
            return Err(CreatureMotionError::InvalidValue);
        }
        Ok(Ray3d::new(
            origin,
            Dir3::new(direction).map_err(|_| CreatureMotionError::InvalidValue)?,
        ))
    }
}
