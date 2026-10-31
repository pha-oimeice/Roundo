//! Generic contact filtering, independent of Items, custody or environment forces.

use avian3d::prelude::{ColliderOf, CollisionHooks, ContactPair, RigidBody};
use bevy::{ecs::system::SystemParam, prelude::*};

/// Restricts this physical root to solid contact with Static rigid bodies.
/// Install `StaticOnlyCollisionHooks` on Avian and enable its pair/contact hooks
/// on the collider. Spatial queries remain independent of solver contact filters.
#[derive(Component)]
pub struct StaticContactsOnly;

/// Composition-owned Avian adapter, not an Item behavior or force implementation.
#[derive(SystemParam)]
pub struct StaticOnlyCollisionHooks<'w, 's> {
    roots: Query<'w, 's, &'static ColliderOf>,
    bodies: Query<'w, 's, &'static RigidBody>,
    restricted: Query<'w, 's, (), With<StaticContactsOnly>>,
}

impl StaticOnlyCollisionHooks<'_, '_> {
    fn root(&self, collider: Entity) -> Entity {
        self.roots
            .get(collider)
            .map_or(collider, |owner| owner.body)
    }

    fn permits(&self, first: Entity, second: Entity) -> bool {
        let a = self.root(first);
        let b = self.root(second);
        let restricted_a = self.restricted.contains(a) || self.restricted.contains(first);
        let restricted_b = self.restricted.contains(b) || self.restricted.contains(second);
        (!restricted_a || self.bodies.get(b) == Ok(&RigidBody::Static))
            && (!restricted_b || self.bodies.get(a) == Ok(&RigidBody::Static))
    }
}

impl CollisionHooks for StaticOnlyCollisionHooks<'_, '_> {
    fn filter_pairs(&self, a: Entity, b: Entity, _: &mut Commands) -> bool {
        self.permits(a, b)
    }

    fn modify_contacts(&self, contacts: &mut ContactPair, _: &mut Commands) -> bool {
        self.permits(contacts.collider1, contacts.collider2)
    }
}
