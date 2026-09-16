//! Authoritative transform representation for local-coordinate entities.

use bevy::{
    prelude::{Changed, Component, IntoScheduleConfigs, Quat, Query, Transform, Vec3},
    transform::TransformSystems,
};

#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(Transform)]
/// Transform copied into Bevy's propagation graph after authoritative changes.
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{prelude::App, transform::TransformPlugin};

    #[test]
    fn engine_transform_is_derived_from_local_coordinate_transform() {
        let mut app = App::new();
        app.add_plugins(TransformPlugin);
        configure_transform_derivation(&mut app);
        let entity = app
            .world_mut()
            .spawn(LocalCoordinateTransform::from_translation(Vec3::new(
                2.0, 3.0, 5.0,
            )))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(2.0, 3.0, 5.0)
        );
    }
}
