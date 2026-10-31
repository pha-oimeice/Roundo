//! Passive world representations and collector-owned, explicit pickup support.
//!
//! Installing this capability creates no items, spheres, actors, inputs or pickup
//! systems. Public custody exists independently. Trusted collector systems decide
//! legality, then use `ItemPickup` in that same synchronous execution. Environment
//! forces, world readiness and integration remain generic physics responsibilities.

use crate::physics::StaticContactsOnly;
use avian3d::prelude::{
    ActiveCollisionHooks, Collider, LinearVelocity, Mass, PhysicsSystems, RigidBody, SpatialQuery,
    SpatialQueryFilter,
};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::*,
};
use roundo_inventory::{Inventories, InventoryError, InventoryHandle, InventoryPlugin, ItemId};
use std::collections::HashMap;

/// Passive correlation only. No events, pickup callback or data authority.
#[derive(Component)]
pub struct DroppedItem {
    item: ItemId,
}
impl DroppedItem {
    pub fn item(&self) -> ItemId {
        self.item
    }
}

/// Validated, unit-scale sphere geometry; the collector supplies its current
/// authoritative center rather than a client hit or cached range qualification.
#[derive(Clone, Copy, Debug)]
pub struct PickupSphere {
    center: Vec3,
    radius: f32,
}
impl PickupSphere {
    pub fn new(center: Vec3, radius: f32) -> Result<Self, WorldItemError> {
        if !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return Err(WorldItemError::InvalidGeometry);
        }
        Ok(Self { center, radius })
    }
}

/// Initial physical description consumed only by the composition adapter.
/// This configures an ordinary Avian body; it implements no gravity/fluid rules.
#[derive(Clone, Copy, Debug)]
pub struct DroppedSphere {
    sphere: PickupSphere,
    mass: f32,
    velocity: Vec3,
}
impl DroppedSphere {
    pub fn new(
        center: Vec3,
        radius: f32,
        mass: f32,
        velocity: Vec3,
    ) -> Result<Self, WorldItemError> {
        if !mass.is_finite() || mass <= 0.0 || !velocity.is_finite() {
            return Err(WorldItemError::InvalidGeometry);
        }
        Ok(Self {
            sphere: PickupSphere::new(center, radius)?,
            mass,
            velocity,
        })
    }
}

/// Recoverable rejections leave custody and registered representations unchanged.
/// Panic/abort/OOM and arbitrary host World mutation are outside this guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldItemError {
    Inventory(InventoryError),
    WrongWorld,
    InvalidGeometry,
    AlreadyRepresented,
    NotRepresented,
    NotPublic,
    InvalidReceiver,
    NotInRange,
    CapacityExceeded,
}
impl From<InventoryError> for WorldItemError {
    fn from(value: InventoryError) -> Self {
        Self::Inventory(value)
    }
}
impl std::fmt::Display for WorldItemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inventory(error) => write!(f, "inventory operation failed: {error}"),
            Self::WrongWorld => f.write_str("world item capability belongs to another World"),
            Self::InvalidGeometry => {
                f.write_str("sphere geometry or initial physics data is invalid")
            }
            Self::AlreadyRepresented => f.write_str("item already has a world representation"),
            Self::NotRepresented => f.write_str("item has no active world representation"),
            Self::NotPublic => f.write_str("item is not held by Public"),
            Self::InvalidReceiver => f.write_str("pickup receiver must differ from Public"),
            Self::NotInRange => {
                f.write_str("item does not intersect the collector's current sphere")
            }
            Self::CapacityExceeded => {
                f.write_str("world item representation capacity is exhausted")
            }
        }
    }
}
impl std::error::Error for WorldItemError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inventory(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Resource)]
struct Representations {
    world: WorldId,
    capacity: usize,
    items: HashMap<ItemId, Entity>,
}

/// Empty architectural capability. Only structural reconciliation is scheduled;
/// pickup remains an explicit action of the collector, not an automatic game rule.
pub struct WorldItemsPlugin {
    capacity: usize,
}
impl WorldItemsPlugin {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "world item representation capacity must be positive"
        );
        Self { capacity }
    }
}
impl Plugin for WorldItemsPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<InventoryPlugin>(),
            "World items require InventoryPlugin"
        );
        assert!(
            !app.world().contains_resource::<Representations>(),
            "world items already installed"
        );
        app.insert_resource(Representations {
            world: app.world().id(),
            capacity: self.capacity,
            items: HashMap::new(),
        })
        .add_systems(FixedPostUpdate, reconcile.before(PhysicsSystems::Prepare));
    }
}

/// Trusted composition access to passive representations. Conflict with
/// `ItemPickup` is scheduler-visible; use separate systems with an explicit order.
#[derive(SystemParam)]
pub struct WorldItems<'w, 's> {
    world: WorldId,
    representations: ResMut<'w, Representations>,
    inventories: Inventories<'w, 's>,
    commands: Commands<'w, 's>,
}
impl WorldItems<'_, '_> {
    /// Gives an already Public item a sphere, without issuing identity, changing
    /// item data or custody. Public items need not call this method at all.
    /// Commands publish the body at the schedule's normal deferred barrier.
    /// Host physics must use `StaticOnlyCollisionHooks`; no collision events are
    /// enabled on this item. Recoverable errors schedule/create nothing.
    pub fn spawn(
        &mut self,
        item: ItemId,
        description: DroppedSphere,
    ) -> Result<Entity, WorldItemError> {
        if self.world != self.representations.world {
            return Err(WorldItemError::WrongWorld);
        }
        let public = self.inventories.public_inventory()?;
        if self.inventories.holder(item)? != Some(public) {
            return Err(WorldItemError::NotPublic);
        }
        if self.representations.items.contains_key(&item) {
            return Err(WorldItemError::AlreadyRepresented);
        }
        if self.representations.items.len() >= self.representations.capacity {
            return Err(WorldItemError::CapacityExceeded);
        }
        let entity = self
            .commands
            .spawn((
                DroppedItem { item },
                RigidBody::Dynamic,
                Collider::sphere(description.sphere.radius),
                Mass(description.mass),
                LinearVelocity(description.velocity),
                Transform::from_translation(description.sphere.center),
                StaticContactsOnly,
                ActiveCollisionHooks::FILTER_PAIRS | ActiveCollisionHooks::MODIFY_CONTACTS,
            ))
            .id();
        self.representations.items.insert(item, entity);
        Ok(entity)
    }

    /// Removes only the world representation, never the item or Public custody.
    pub fn remove(&mut self, item: ItemId) -> Result<(), WorldItemError> {
        if self.world != self.representations.world {
            return Err(WorldItemError::WrongWorld);
        }
        self.inventories.public_inventory()?;
        let entity = self
            .representations
            .items
            .remove(&item)
            .ok_or(WorldItemError::NotRepresented)?;
        self.commands.entity(entity).try_despawn();
        Ok(())
    }
}

/// Collector-side collision checking and immediate custody commit, not behavior
/// of the object. No system is automatically installed to invoke these methods.
///
/// The caller resolves the actor's current physical range, receiver association,
/// same-space qualification and business legality in this same system. Merely
/// possessing an ItemId/range is not player authority. Current collision and
/// source custody are checked here again before synchronous inventory transfer.
/// Schedule after generic physics has synchronized the current collider query
/// trees/poses. Moving a target without that synchronization is not supported;
/// this parameter does not advance or rebuild the physics world itself.
#[derive(SystemParam)]
pub struct ItemPickup<'w, 's> {
    world: WorldId,
    representations: ResMut<'w, Representations>,
    inventories: Inventories<'w, 's>,
    spatial: SpatialQuery<'w, 's>,
    dropped: Query<'w, 's, &'static DroppedItem>,
    commands: Commands<'w, 's>,
}
impl ItemPickup<'_, '_> {
    /// Owned sorted candidate identities, not authorization or cached hit proofs.
    /// Only exact intersections of currently Public registered bodies are returned.
    pub fn candidates(&self, range: PickupSphere) -> Result<Vec<ItemId>, WorldItemError> {
        if self.world != self.representations.world {
            return Err(WorldItemError::WrongWorld);
        }
        let public = self.inventories.public_inventory()?;
        let mut result = Vec::new();
        self.spatial.shape_intersections_callback(
            &Collider::sphere(range.radius),
            range.center,
            Quat::IDENTITY,
            &SpatialQueryFilter::default(),
            |entity| {
                if let Ok(drop) = self.dropped.get(entity)
                    && self.representations.items.get(&drop.item) == Some(&entity)
                    && self.inventories.holder(drop.item) == Ok(Some(public))
                {
                    result.push(drop.item);
                }
                true
            },
        );
        result.sort_unstable();
        result.dedup();
        Ok(result)
    }

    /// The collector calls this only after its own legality checks. It rechecks
    /// current exact collision and Public custody, then commits immediately.
    /// Success closes the representation synchronously; deferred despawn cannot
    /// permit a second pickup. No events, animation, Portal or next-tick queue.
    pub fn take(
        &mut self,
        item: ItemId,
        range: PickupSphere,
        receiver: InventoryHandle,
    ) -> Result<(), WorldItemError> {
        if self.world != self.representations.world {
            return Err(WorldItemError::WrongWorld);
        }
        let public = self.inventories.public_inventory()?;
        if receiver == public {
            return Err(WorldItemError::InvalidReceiver);
        }
        let entity = *self
            .representations
            .items
            .get(&item)
            .ok_or(WorldItemError::NotRepresented)?;
        if !self.dropped.get(entity).is_ok_and(|drop| drop.item == item) {
            return Err(WorldItemError::NotRepresented);
        }
        if self.inventories.holder(item)? != Some(public) {
            return Err(WorldItemError::NotPublic);
        }
        let mut intersects = false;
        self.spatial.shape_intersections_callback(
            &Collider::sphere(range.radius),
            range.center,
            Quat::IDENTITY,
            &SpatialQueryFilter::default(),
            |hit| {
                intersects |= hit == entity;
                !intersects
            },
        );
        if !intersects {
            return Err(WorldItemError::NotInRange);
        }
        self.inventories.transfer(public, receiver, item)?;
        self.representations.items.remove(&item);
        self.commands.entity(entity).try_despawn();
        Ok(())
    }
}

// Reconciliation is structural, not a gameplay pickup loop: direct host custody
// changes/despawns cannot leave a persistent world obstacle. Exact entity checks
// protect unrelated/replacement objects. Public custody itself is never repaired
// by manufacturing items, transferring them back or inventing a position.
fn reconcile(
    world: WorldId,
    mut representations: ResMut<Representations>,
    inventories: Inventories,
    dropped: Query<&DroppedItem>,
    mut commands: Commands,
) {
    if world != representations.world {
        return;
    }
    let Ok(public) = inventories.public_inventory() else {
        return;
    };
    representations.items.retain(|item, entity| {
        if !dropped.get(*entity).is_ok_and(|drop| drop.item == *item) {
            return false;
        }
        if inventories.holder(*item) != Ok(Some(public)) {
            commands.entity(*entity).try_despawn();
            return false;
        }
        true
    });
}
