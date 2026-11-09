//! Typed, immediate Life validation and exact post-invalidation cleanup.

use crate::{InventoryError, InventoryHandle, InventoryOwner, ItemId, store::InventoryStore};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::{App, On, Plugin, ResMut},
};
use roundo_life::{
    ConceptBodyFactory, ConceptBodyHandle, FactoryInvalidation, LifeInstances, LifeRuntimePlugin,
    with_life_administration,
};

type Result<T> = std::result::Result<T, InventoryError>;

/// Installs inventory capability after [`LifeRuntimePlugin`] in the same App.
///
/// Creates the one World-lifetime Public Concept Body and its inventory before
/// ordinary inventory use. No item entities, gameplay entry points, typed data or
/// ticks are created. The catalogue need not be closed and is not changed.
/// Missing Life, duplicate installation, or store identity exhaustion rejects
/// before creating Public. Exact dead-owner cleanup spills only direct items to
/// Public; notifications are not credentials and live Bodies remain protected.
#[derive(Default)]
pub struct InventoryPlugin;

fn check_installation(app: &App) {
    assert!(
        app.is_plugin_added::<LifeRuntimePlugin>(),
        "InventoryPlugin requires LifeRuntimePlugin to be installed first"
    );
    assert!(
        !app.world().contains_resource::<InventoryStore>(),
        "InventoryPlugin refuses to replace an existing inventory store"
    );
}

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        check_installation(app);
        let store = InventoryStore::try_new(app.world().id());
        install_inventory(app, store);
    }
}

// Publish only after store identity and Public initialization succeed; not a recovery API.
fn install_inventory(app: &mut App, store: Result<InventoryStore>) {
    check_installation(app);
    let mut store =
        store.expect("InventoryPlugin identity allocation failed; no inventory published");
    store
        .check_world(app.world().id())
        .expect("InventoryPlugin store belongs to another World");
    let public = with_life_administration(app.world_mut(), |world, admin| {
        ConceptBodyFactory::create_world_root(world, admin)
    })
    .expect("Public Concept Body allocation failed");
    store
        .install_public(public)
        .expect("Public inventory allocation failed");
    app.insert_resource(store)
        .add_observer(clear_invalidated_owner);
}

fn clear_invalidated_owner(
    event: On<FactoryInvalidation>,
    world: WorldId,
    life: LifeInstances,
    store: Option<ResMut<InventoryStore>>,
) {
    let Some(mut store) = store else { return };
    if store.check_world(world).is_err() {
        return;
    }
    let FactoryInvalidation::ConceptBody(body) = *event else {
        return;
    };
    // Events can be copied/constructed and have no World/epoch provenance.
    // Current exact liveness, not event origin, protects live owners.
    if life.resolve_body(body).is_ok() {
        return;
    }
    store.clear_owner(body);
}

/// Trusted local, immediate inventory access; not Player or disclosure authority.
///
/// Requires the installed resource even for reads and exclusively borrows it, so
/// structural commits are serial. Every call first rechecks the current World,
/// then domain/identity, then exact Body liveness before any custody checks/write.
/// Handles/owners are valid only in this World and store epoch while the exact Body
/// remains live. Removal invalidates a handle; store rebuild invalidates all old
/// handles/owners; Factory invalidation rejects normal access immediately, even if
/// cleanup is delayed. No membership in a Life or typed data permission is required.
///
/// IDs are caller-supplied identities unique among live items, not issued here.
/// A reused number alone cannot distinguish instances; callers must separately
/// validate exact item identity before acting on a retained request.
/// All returned `Err`s leave inventory structure unchanged. Checks and synchronous
/// commits run without external rules, callbacks, async work or clock advancement;
/// this is not a transaction for external data, side effects, panic/abort or OOM.
/// Snapshots are owned, unordered, do not track updates and grant no disclosure right.
#[derive(SystemParam)]
pub struct Inventories<'w, 's> {
    world: WorldId,
    life: LifeInstances<'w, 's>,
    store: ResMut<'w, InventoryStore>,
}

impl Inventories<'_, '_> {
    fn live(&self, body: ConceptBodyHandle) -> Result<()> {
        self.life
            .resolve_body(body)
            .map(|_| ())
            .map_err(|_| InventoryError::OwnerNotLive)
    }

    fn check_owner(&self, owner: InventoryOwner) -> Result<()> {
        if owner.domain() != self.store.domain() {
            return Err(InventoryError::WrongDomain);
        }
        self.live(owner.body())
    }

    fn check_inventory(&self, handle: InventoryHandle) -> Result<()> {
        let owner = self.store.owner(handle)?;
        self.live(owner.body())
    }

    /// The unique unrestricted fallback inventory of this World/store.
    /// Public custody does not imply a physical representation or data grant.
    pub fn public_inventory(&self) -> Result<InventoryHandle> {
        self.store.check_world(self.world)?;
        let handle = self.store.public()?;
        self.check_inventory(handle)?;
        Ok(handle)
    }

    /// Public is a real, World-lifetime Concept Body, not an ownerless sentinel.
    /// The opaque owner may be queried, but cannot create extra Public inventories.
    pub fn public_owner(&self) -> Result<InventoryOwner> {
        self.store.owner(self.public_inventory()?)
    }

    /// Returns this inventory's owner after World, identity and exact Body checks.
    /// The returned identity grants no Player access and can become stale later.
    pub fn owner(&self, handle: InventoryHandle) -> Result<InventoryOwner> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.owner(handle)
    }

    /// Owned unordered snapshot of all inventories, including Public.
    /// Checks World and every exact owner's liveness; any invalid entry fails the
    /// entire read without repairing or silently omitting it. O(allocated slots).
    /// This trusted-local enumeration is not permission to disclose the result.
    pub fn all(&self) -> Result<Vec<InventoryHandle>> {
        self.store.check_world(self.world)?;
        let handles = self.store.all();
        for handle in &handles {
            self.check_inventory(*handle)?;
        }
        Ok(handles)
    }

    /// Selects an exact live Body in this trusted current World.
    /// World mismatch is `WrongDomain`, failed resolution `OwnerNotLive`.
    /// A bare Body has no World field: binding does not authenticate its origin
    /// or confer Player authorization. The owned context is tied to this store epoch.
    pub fn bind_owner(&self, body: ConceptBodyHandle) -> Result<InventoryOwner> {
        self.store.check_world(self.world)?;
        self.live(body)?;
        Ok(InventoryOwner::new(self.store.domain(), body))
    }

    /// Immediately registers an empty inventory for a local live owner.
    /// Checks World, owner domain (`WrongDomain`), then Body (`OwnerNotLive`),
    /// then allocates identity (`IdentityExhausted` with no partial registration).
    /// Multiple independent inventories per Body are allowed; no Entity is spawned.
    pub fn create(&mut self, owner: InventoryOwner) -> Result<InventoryHandle> {
        self.store.check_world(self.world)?;
        self.check_owner(owner)?;
        self.store.create(owner)
    }

    /// Retires an ordinary inventory and transfers its direct items to Public.
    /// Checks World, identity, then Body liveness; Public removal is rejected.
    /// Does not alter the items, their own inventories, IDs/data, or spawn physics.
    /// Cost is proportional to direct members; `Err` changes nothing.
    pub fn remove(&mut self, handle: InventoryHandle) -> Result<()> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.remove(handle)
    }

    /// Immediately records an externally supplied ID, without reading item data.
    /// World -> H domain/structure -> Body live -> unheld ID; duplicate custody,
    /// including in this same inventory, is `ItemAlreadyHeld`. Identity/liveness
    /// errors take priority; `Err` changes nothing. Expected O(1), no Entity spawn.
    pub fn insert(&mut self, handle: InventoryHandle, item: ItemId) -> Result<()> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.insert(handle, item)
    }

    /// Immediately throws a direct item into Public without changing its data.
    /// Taking from Public itself validates custody then succeeds unchanged.
    /// World -> H identity -> Body live -> source custody. `Err` changes nothing;
    /// expected O(1). No physical representation or nested traversal is created.
    pub fn take(&mut self, handle: InventoryHandle, item: ItemId) -> Result<()> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.take(handle, item)
    }

    /// Removes the final custody entry during trusted item destruction, including
    /// from Public. Unlike `take`, this never transfers the item to another owner.
    ///
    /// This is only the inventory part of a terminal lifecycle commit: the caller
    /// must validate the exact item instance and end its lifetime in the same
    /// serialized operation. Never use this to detach a still-live item. Checks
    /// World, inventory identity, live owner and source membership before writing;
    /// any `Err` leaves custody unchanged. Does not reserve or blacklist the ID.
    pub fn remove_destroyed_item(&mut self, handle: InventoryHandle, item: ItemId) -> Result<()> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.remove_destroyed_item(handle, item)
    }

    /// Atomically and immediately moves custody and both member sets, expected O(1).
    /// Order: World -> source structure -> target structure -> source live ->
    /// target live -> source custody. Thus both structural identities precede Life
    /// checks; `WrongDomain` precedes `InvalidInventory` within each identity.
    /// Same-inventory transfers still perform every check, then succeed unchanged.
    /// `ItemNotHeld` includes unregistered and other-holder IDs. Any `Err` leaves
    /// structure unchanged; no capacity/type/rules or external effects are transacted.
    pub fn transfer(
        &mut self,
        source: InventoryHandle,
        target: InventoryHandle,
        item: ItemId,
    ) -> Result<()> {
        self.store.check_world(self.world)?;
        let source_owner = self.store.owner(source)?;
        let target_owner = self.store.owner(target)?;
        self.live(source_owner.body())?;
        self.live(target_owner.body())?;
        self.store.transfer(source, target, item)
    }

    /// Owned unordered ID snapshot, O(output); not a live view or disclosure grant.
    /// World -> H domain/structure -> Body live; errors are not silently cleaned.
    /// Does not change custody; later mutations do not change this Vec.
    pub fn items(&self, handle: InventoryHandle) -> Result<Vec<ItemId>> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.items(handle)
    }

    /// Current direct member count, O(1), after World, identity and Body checks.
    /// Not the Capacity or Occupancy attribute and not a capacity restriction.
    pub fn item_count(&self, handle: InventoryHandle) -> Result<usize> {
        self.store.check_world(self.world)?;
        self.check_inventory(handle)?;
        self.store.item_count(handle)
    }

    /// Expected O(1) indexed lookup after the World check. No registration gives
    /// `Ok(None)` without inserting anything. A registered H is structurally
    /// validated, then its exact Body: delayed dead-owner custody is
    /// `Err(OwnerNotLive)`, never disguised as None or silently cleaned.
    pub fn holder(&self, item: ItemId) -> Result<Option<InventoryHandle>> {
        self.store.check_world(self.world)?;
        let holder = self.store.holder(item)?;
        if let Some(handle) = holder {
            self.check_inventory(handle)?;
        }
        Ok(holder)
    }

    /// Owned unordered inventory snapshot for this exact owner, O(output).
    /// World -> owner domain -> exact Body live; live Bodies without inventories
    /// return an empty Vec. No scanning/cleaning of other owners. Snapshot does not
    /// follow later registrations and grants neither validity nor disclosure rights.
    pub fn for_body(&self, owner: InventoryOwner) -> Result<Vec<InventoryHandle>> {
        self.store.check_world(self.world)?;
        self.check_owner(owner)?;
        self.store.for_body(owner)
    }

    /// O(1) read-only structural registration count after checking World.
    /// Does not inspect/clean owners: delayed invalid registrations still count.
    /// This is not evidence that all inventories are live. Normal Factory cleanup
    /// has already reduced the count when removal returns.
    pub fn len(&self) -> Result<usize> {
        self.store.check_world(self.world)?;
        Ok(self.store.len())
    }
}
