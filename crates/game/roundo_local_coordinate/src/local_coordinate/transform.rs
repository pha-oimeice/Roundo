//! Derives engine transforms from authoritative spatial attributes.

use super::attributes::LocalCoordinateTransform;
use bevy::{
    prelude::{Changed, IntoScheduleConfigs, Query, Transform},
    transform::TransformSystems,
};
/// Updates engine transforms before hierarchy propagation.
pub(crate) fn derive_bevy_transforms(
    mut transforms: Query<
        (&LocalCoordinateTransform, &mut Transform),
        Changed<LocalCoordinateTransform>,
    >,
) {
    for (local_coordinate_transform, mut transform) in &mut transforms {
        *transform = local_coordinate_transform.as_bevy();
    }
}

/// Orders authoritative transform derivation in the post-update schedule.
pub(crate) fn configure_transform_derivation(app: &mut bevy::prelude::App) {
    app.add_systems(
        bevy::prelude::PostUpdate,
        derive_bevy_transforms.before(TransformSystems::Propagate),
    );
}
