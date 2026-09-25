//! Private Avian adapter.  Avian's character-controller types never cross the
//! Creature public API boundary.

use avian3d::{
    math::{AdjustPrecision as _, AsF32 as _},
    prelude::{
        Collider, MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitResponse, SpatialQueryFilter,
    },
};
use bevy::prelude::{Entity, Quat, Vec3};

pub(crate) struct CollisionResult {
    pub(crate) position: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) normals: Vec<Vec3>,
}

pub(crate) fn move_capsule(
    mover: &MoveAndSlide,
    entity: Entity,
    capsule: &Collider,
    position: Vec3,
    rotation: Quat,
    velocity: Vec3,
    dt: std::time::Duration,
) -> CollisionResult {
    let mut normals = Vec::new();
    let mut config = MoveAndSlideConfig::default();
    // Avian defaults to 1 cm, which is visible at Creature scale and allows
    // grounded snapshots to alternate across that entire separation. A 1 mm
    // skin retains numerical separation without producing camera-height jitter.
    config.skin_width = 0.001;
    let output = mover.move_and_slide(
        capsule,
        position.adjust_precision(),
        rotation.adjust_precision(),
        velocity.adjust_precision(),
        dt,
        &config,
        &SpatialQueryFilter::from_excluded_entities([entity]),
        |hit| {
            normals.push(hit.normal.f32());
            MoveAndSlideHitResponse::Accept
        },
    );
    CollisionResult {
        position: output.position.f32(),
        velocity: output.projected_velocity.f32(),
        normals,
    }
}
