use bevy::prelude::{Quat, Vec3};

/// Projects gaze-local horizontal input to a support plane.  When gaze points
/// into the support normal, the stored body heading is the deterministic fallback.
pub(crate) fn surface_direction(
    local_axis: Vec3,
    gaze: Quat,
    support: Vec3,
    body_heading: Vec3,
) -> Vec3 {
    let support = support.normalize_or_zero();
    let desired = gaze * Vec3::new(local_axis.x, 0.0, -local_axis.z);
    let tangent = desired - support * desired.dot(support);
    if tangent.length_squared() > f32::EPSILON {
        tangent.normalize()
    } else {
        (body_heading - support * body_heading.dot(support)).normalize_or_zero()
    }
}

pub(crate) fn surface_is_legal(force: Vec3, normal: Vec3) -> bool {
    if force.length_squared() <= f32::EPSILON {
        return false;
    }
    normal.normalize_or_zero().dot(-force.normalize()) >= std::f32::consts::FRAC_PI_6.cos()
}

pub(crate) fn snap_distance(tangent_distance: f32, contact_tolerance: f32) -> f32 {
    tangent_distance.max(0.0) * std::f32::consts::FRAC_PI_6.tan() + contact_tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_force_cannot_be_grounded() {
        assert!(!surface_is_legal(Vec3::ZERO, Vec3::Y));
    }
    #[test]
    fn slope_boundary_is_legal() {
        assert!(surface_is_legal(-Vec3::Y, Vec3::new(0.5, 0.866_025_4, 0.0)));
    }
    #[test]
    fn positive_local_forward_uses_gaze_negative_z() {
        assert_eq!(
            surface_direction(Vec3::Z, Quat::IDENTITY, Vec3::Y, Vec3::NEG_Z),
            Vec3::NEG_Z
        );
        assert_eq!(
            surface_direction(Vec3::NEG_Z, Quat::IDENTITY, Vec3::Y, Vec3::NEG_Z),
            Vec3::Z
        );
    }
}
