//! Pure-ID custody for trusted local ECS systems, not Player authorization.
//!
//! Install [`InventoryPlugin`] after `roundo_life::LifeRuntimePlugin`. It creates
//! one real World-lifetime Public Concept Body and its unique inventory, without
//! closing the Life catalogue, advancing clocks or spawning item/gameplay entities.
//! Public cannot be removed by normal Factory or inventory operations and exists
//! until the World is dropped. Ordinary Bodies still receive no automatic inventory.
//!
//! [`ItemId::from_u128`] imports an existing identity; copying it does not issue
//! an item. Callers ensure uniqueness among live items and do not simultaneously
//! custody one actual item in independent authorities. Reused numbers require
//! separate exact-instance validation by the caller. Item data remains external.
//!
//! [`InventoryHandle`] and [`InventoryOwner`] are opaque copied identities, not
//! authorization. Access validates current World + store epoch, exact inventory
//! generation and live Body. Binding a bare Body selects this World's instance,
//! not its source provenance. Public has the same domain/liveness checks.
//!
//! [`Inventories`] declares exclusive store access even for queries. Transfer
//! checks World, source structure, target structure, source live, target live,
//! then custody (also for same-inventory transfers). Structure errors change
//! nothing; panic/abort/OOM and external effects are outside that guarantee.
//!
//! `take`, ordinary inventory removal and exact dead-Body cleanup move direct
//! members to Public, never into an ownerless gap. If A holds B and B holds C,
//! A does not hold C: spilling B does not mutate B, its data or its own inventories.
//! Those operations perform no nested traversal, item destruction, physics spawn,
//! Item event, automatic pickup, data grant or network output. Public custody need
//! not have any spatial representation. A Public `take` validates custody and is
//! a no-op. `remove_destroyed_item` is a separate terminal-cleanup interface: it
//! removes custody without spilling, only as part of caller-coordinated item
//! destruction, never as a way to leave a live item without an owner.
//!
//! Queries return owned unordered snapshots, not live views/disclosure rights.
//! `len` includes Public and any not-yet-cleaned dead-owner inventories. `holder`
//! distinguishes never-held `None` from delayed dead-owner `OwnerNotLive`; normal
//! Factory completion has already spilled the dead owner's items to Public.
//! Cleanup is targeted by exact Body, World-checked and idempotent; copied
//! notifications cannot remove a currently live Body's inventories. Trusted raw
//! World/resource bypass is not sandboxed or repaired by a global scan.
//!
//! `PublicProtected` rejects Public removal or another inventory for its owner;
//! `PublicUnavailable` identifies a host-bypassed/uninstalled private core. Other
//! error meanings and identity checks are defined on [`InventoryError`].

pub mod attributes;
mod id;
mod life_adapter;
mod store;

pub use attributes::{Capacity, Occupancy};
pub use id::{InventoryError, InventoryHandle, InventoryOwner, ItemId};
pub use life_adapter::{Inventories, InventoryPlugin};
