//! Discrete, whole-object teleportation through block-attached portals.
//!
//! A portal is represented by a thin sensor collider carrying [`Portal`]. It
//! can be parented to the entity that owns the host block. Teleportable objects
//! opt in by placing [`AtomicTeleportableObject`] on their root entity (the
//! rigid-body entity when physics is involved). Voxel local coordinates and
//! their chunks deliberately do not opt in.
//!
//! Endpoint scale is intentionally excluded from teleport mapping. Translation
//! and rotation form a rigid mapping, so mass, shape, damping, and other body
//! properties remain unchanged while linear and angular velocities rotate into
//! the destination frame.

pub mod attributes;
pub use attributes::{AtomicTeleportableObject, Portal};

use avian3d::prelude::{
    AngularVelocity, CollisionStart, Collisions, LinearVelocity, PhysicsSchedule,
    PhysicsStepSystems, Position, RigidBodyColliders, Rotation,
};
use bevy::prelude::{
    App, ChildOf, Commands, Component, Entity, GlobalTransform, IntoScheduleConfigs, MessageReader,
    Plugin, Quat, Query, Transform, Vec3, With,
};
use std::collections::HashSet;
use std::f32::consts::PI;

/// Installs authoritative portal teleportation into Avian's physics schedule.
///
/// Add this after `avian3d::PhysicsPlugins`.
pub struct RoundoPortalPlugin;

impl Plugin for RoundoPortalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PhysicsSchedule,
            (release_departed_objects, teleport_entering_objects)
                .chain()
                .in_set(PhysicsStepSystems::Last),
        );
    }
}

/// Prevents an object mapped into the destination sensor from immediately
/// traversing the same portal pair in reverse.
#[derive(Component)]
struct PortalExitGuard {
    portal: Entity,
}

fn release_departed_objects(
    mut commands: Commands,
    collisions: Collisions,
    guarded_objects: Query<
        (Entity, &PortalExitGuard, Option<&RigidBodyColliders>),
        With<AtomicTeleportableObject>,
    >,
) {
    for (object, guard, rigid_body_colliders) in &guarded_objects {
        let still_inside_destination = collisions.contains(object, guard.portal)
            || rigid_body_colliders.is_some_and(|colliders| {
                colliders
                    .into_iter()
                    .any(|collider| collisions.contains(collider, guard.portal))
            });

        if !still_inside_destination {
            commands.entity(object).remove::<PortalExitGuard>();
        }
    }
}

#[allow(clippy::type_complexity)]
fn teleport_entering_objects(
    mut commands: Commands,
    mut collision_starts: MessageReader<CollisionStart>,
    portals: Query<(&Portal, &GlobalTransform)>,
    parent_transforms: Query<&GlobalTransform>,
    tagged_objects: Query<(), With<AtomicTeleportableObject>>,
    exit_guards: Query<&PortalExitGuard>,
    mut objects: Query<(
        &GlobalTransform,
        &mut Transform,
        Option<&ChildOf>,
        Option<&mut Position>,
        Option<&mut Rotation>,
        Option<&mut LinearVelocity>,
        Option<&mut AngularVelocity>,
    )>,
) {
    let mut teleported_this_step = HashSet::new();

    let started_collisions = collision_starts.read();
    for collision in started_collisions {
        let Some((source_entity, object_entity)) = portal_collision(collision, &portals) else {
            continue;
        };
        if !tagged_objects.contains(object_entity)
            || teleported_this_step.contains(&object_entity)
            || {
                let exit_guard = exit_guards.get(object_entity);
                exit_guard.is_ok_and(|guard| guard.portal == source_entity)
            }
        {
            continue;
        }

        let Ok((source, source_transform)) = portals.get(source_entity) else {
            continue;
        };
        let destination_entity = source.destination();
        let Ok((_, destination_transform)) = portals.get(destination_entity) else {
            continue;
        };
        let Ok((
            current_global_transform,
            mut transform,
            parent,
            position,
            rotation,
            linear_velocity,
            angular_velocity,
        )) = objects.get_mut(object_entity)
        else {
            continue;
        };

        let mapping = PortalMapping::between(source_transform, destination_transform);
        let current_world_transform = current_global_transform.compute_transform();
        let current_translation = position
            .as_deref()
            .map_or(current_world_transform.translation, |position| position.0);
        let current_rotation = rotation
            .as_deref()
            .map_or(current_world_transform.rotation, |rotation| {
                Quat::from(*rotation)
            });
        let mapped_translation = mapping.map_point(current_translation);
        let mapped_rotation = mapping.map_rotation(current_rotation);
        teleported_this_step.insert(object_entity);

        let mut mapped_world_transform = current_world_transform;
        mapped_world_transform.translation = mapped_translation;
        mapped_world_transform.rotation = mapped_rotation;
        *transform = parent
            .and_then(|parent| {
                let parent_transform = parent_transforms.get(parent.parent());
                parent_transform.ok()
            })
            .map_or(mapped_world_transform, |parent_transform| {
                GlobalTransform::from(mapped_world_transform).reparented_to(parent_transform)
            });

        if let Some(mut position) = position {
            position.0 = mapped_translation;
        }
        if let Some(mut rotation) = rotation {
            *rotation = Rotation::from(mapped_rotation);
        }
        if let Some(mut linear_velocity) = linear_velocity {
            linear_velocity.0 = mapping.map_vector(linear_velocity.0);
        }
        if let Some(mut angular_velocity) = angular_velocity {
            angular_velocity.0 = mapping.map_vector(angular_velocity.0);
        }

        commands.entity(object_entity).insert(PortalExitGuard {
            portal: destination_entity,
        });
    }
}

fn portal_collision(
    collision: &CollisionStart,
    portals: &Query<(&Portal, &GlobalTransform)>,
) -> Option<(Entity, Entity)> {
    if portals.contains(collision.collider1) {
        return Some((
            collision.collider1,
            collision.body2.unwrap_or(collision.collider2),
        ));
    }
    if portals.contains(collision.collider2) {
        return Some((
            collision.collider2,
            collision.body1.unwrap_or(collision.collider1),
        ));
    }
    None
}

#[derive(Clone, Copy, Debug)]
struct PortalMapping {
    source_translation: Vec3,
    destination_translation: Vec3,
    rotation: Quat,
}

impl PortalMapping {
    fn between(source: &GlobalTransform, destination: &GlobalTransform) -> Self {
        let (_, source_rotation, source_translation) = source.to_scale_rotation_translation();
        let (_, destination_rotation, destination_translation) =
            destination.to_scale_rotation_translation();
        let turn_through_portal = Quat::from_rotation_y(PI);

        Self {
            source_translation,
            destination_translation,
            rotation: destination_rotation * turn_through_portal * source_rotation.inverse(),
        }
    }

    fn map_point(self, point: Vec3) -> Vec3 {
        self.destination_translation + self.map_vector(point - self.source_translation)
    }

    fn map_rotation(self, rotation: Quat) -> Quat {
        (self.rotation * rotation).normalize()
    }

    fn map_vector(self, vector: Vec3) -> Vec3 {
        self.rotation * vector
    }
}
