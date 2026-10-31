//! Native portal capability and endpoint attributes, separate from teleport work.
//!
//! These values opt an object into a behavior; attaching them does not acquire
//! player control, load content or change World/session ownership.

use avian3d::prelude::{CollisionEventsEnabled, Sensor};
use bevy::prelude::{Component, Entity, Transform};

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
