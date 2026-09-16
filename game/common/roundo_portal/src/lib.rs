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

use avian3d::prelude::{
    AngularVelocity, CollisionEventsEnabled, CollisionStart, Collisions, LinearVelocity,
    PhysicsSchedule, PhysicsStepSystems, Position, RigidBodyColliders, Rotation, Sensor,
};
use bevy::prelude::{
    App, ChildOf, Commands, Component, Entity, GlobalTransform, IntoScheduleConfigs, MessageReader,
    Plugin, Quat, Query, Transform, Vec3, With,
};
use std::collections::HashSet;
use std::f32::consts::PI;

/// Opts an entire object into discrete portal teleportation.
///
/// Put this marker on the object's root entity. For a compound rigid body,
/// place it on the rigid-body entity rather than on an individual collider.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[require(Transform)]
pub struct AtomicTeleportableObject;

/// A block-attached portal entrance linked to another portal entity.
///
/// The entity must also have a collider describing the entrance volume. The
/// required sensor and collision-event components make that collider a trigger
/// rather than a physical obstacle. Parent the entity to its host block (or to
/// the entity owning that block) to attach the logic spatially.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[require(Transform, Sensor, CollisionEventsEnabled)]
pub struct Portal {
    destination: Entity,
}

impl Portal {
    pub const fn new(destination: Entity) -> Self {
        Self { destination }
    }

    pub const fn destination(self) -> Entity {
        self.destination
    }

    pub fn set_destination(&mut self, destination: Entity) {
        self.destination = destination;
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::{
        PhysicsPlugins,
        collision::contact_types::ContactGraph,
        prelude::{Mass, RigidBody},
    };
    use bevy::{MinimalPlugins, prelude::App, transform::TransformPlugin};

    #[test]
    fn atomic_teleportable_object_is_an_explicit_transform_bearing_tag() {
        let mut app = App::new();
        let entity = app.world_mut().spawn(AtomicTeleportableObject).id();

        assert!(app.world().entity(entity).contains::<Transform>());
        assert!(app.world().entity(entity).contains::<GlobalTransform>());
    }

    #[test]
    fn portal_mapping_is_a_rigid_half_turn_between_endpoints() {
        let source = GlobalTransform::from(Transform::from_xyz(1.0, 0.0, 0.0));
        let destination = GlobalTransform::from(
            Transform::from_xyz(10.0, 2.0, 0.0).with_rotation(Quat::from_rotation_y(PI / 2.0)),
        );
        let mapping = PortalMapping::between(&source, &destination);

        assert_vec3_close(
            mapping.map_point(Vec3::new(1.0, 0.0, 2.0)),
            Vec3::new(8.0, 2.0, 0.0),
        );
        assert_vec3_close(mapping.map_vector(Vec3::Z), -Vec3::X);
        assert!(
            (mapping.map_vector(Vec3::new(2.0, 3.0, 5.0)).length()
                - Vec3::new(2.0, 3.0, 5.0).length())
            .abs()
                < 1e-5
        );
    }

    #[test]
    fn collision_teleports_once_and_preserves_dynamic_body_properties() {
        let mut app = App::new();
        app.add_message::<CollisionStart>()
            .init_resource::<ContactGraph>()
            .add_plugins(RoundoPortalPlugin);

        let destination_transform = Transform::from_xyz(10.0, 0.0, 0.0);
        let destination = app
            .world_mut()
            .spawn((
                Portal::new(Entity::PLACEHOLDER),
                destination_transform,
                GlobalTransform::from(destination_transform),
            ))
            .id();
        let source = app
            .world_mut()
            .spawn((
                Portal::new(destination),
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.world_mut()
            .entity_mut(destination)
            .insert(Portal::new(source));

        let object = app
            .world_mut()
            .spawn((
                AtomicTeleportableObject,
                RigidBody::Dynamic,
                Mass(7.0),
                Transform::from_xyz(0.0, 0.0, 1.0),
                GlobalTransform::from(Transform::from_xyz(0.0, 0.0, 1.0)),
                Position(Vec3::new(0.0, 0.0, 1.0)),
                Rotation::default(),
                LinearVelocity(Vec3::new(0.0, 0.0, -4.0)),
                AngularVelocity(Vec3::new(0.0, 2.0, 0.0)),
            ))
            .id();

        app.world_mut().write_message(CollisionStart {
            collider1: source,
            collider2: object,
            body1: None,
            body2: Some(object),
        });
        app.world_mut().write_message(CollisionStart {
            collider1: destination,
            collider2: object,
            body1: None,
            body2: Some(object),
        });
        app.world_mut().run_schedule(PhysicsSchedule);

        let object_ref = app.world().entity(object);
        assert_vec3_close(
            object_ref.get::<Position>().unwrap().0,
            Vec3::new(10.0, 0.0, -1.0),
        );
        assert_vec3_close(
            object_ref.get::<LinearVelocity>().unwrap().0,
            Vec3::new(0.0, 0.0, 4.0),
        );
        assert_vec3_close(
            object_ref.get::<AngularVelocity>().unwrap().0,
            Vec3::new(0.0, 2.0, 0.0),
        );
        assert_eq!(*object_ref.get::<RigidBody>().unwrap(), RigidBody::Dynamic);
        assert_eq!(object_ref.get::<Mass>().unwrap().0, 7.0);
        assert!(object_ref.contains::<PortalExitGuard>());
    }

    #[test]
    fn plugin_is_ordered_in_the_complete_physics_schedule() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            PhysicsPlugins::default(),
            RoundoPortalPlugin,
        ));
        app.finish();

        app.world_mut().run_schedule(PhysicsSchedule);
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        assert!(
            actual.abs_diff_eq(expected, 1e-5),
            "expected {expected:?}, got {actual:?}"
        );
    }
}
