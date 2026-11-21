//! Private structural core. All normal access must go through a Life adapter.
//! No callbacks, rules, external item data, capacity policy, or liveness claims.

use crate::{InventoryError, InventoryHandle, InventoryOwner, ItemId, id::InventoryDomain};
use bevy::{ecs::world::WorldId, prelude::Resource};
use roundo_life::ConceptBodyHandle;
use std::collections::{HashMap, HashSet};

type Result<T> = std::result::Result<T, InventoryError>;

#[derive(Debug)]
struct Entry {
    body: ConceptBodyHandle,
    items: HashSet<ItemId>,
}

#[derive(Debug)]
struct Slot {
    generation: u64,
    entry: Option<Entry>,
}

/// One authoritative custody domain. Exclusive access serializes structural
/// commits; checks happen before writes. This resource is deliberately not Clone.
#[derive(Resource)]
pub(crate) struct InventoryStore {
    domain: InventoryDomain,
    slots: Vec<Slot>,
    free: Vec<u32>,
    holders: HashMap<ItemId, InventoryHandle>,
    owners: HashMap<ConceptBodyHandle, HashSet<InventoryHandle>>,
    live_count: usize,
    public: Option<InventoryHandle>,
}

impl InventoryStore {
    pub(crate) fn try_new(world: WorldId) -> Result<Self> {
        Ok(Self {
            domain: InventoryDomain::try_new(world)?,
            slots: Vec::new(),
            free: Vec::new(),
            holders: HashMap::new(),
            owners: HashMap::new(),
            live_count: 0,
            public: None,
        })
    }

    pub(crate) fn domain(&self) -> InventoryDomain {
        self.domain
    }

    pub(crate) fn install_public(&mut self, body: ConceptBodyHandle) -> Result<()> {
        if self.public.is_some() {
            return Err(InventoryError::PublicProtected);
        }
        self.public = Some(self.create(InventoryOwner::new(self.domain, body))?);
        Ok(())
    }

    pub(crate) fn public(&self) -> Result<InventoryHandle> {
        self.public.ok_or(InventoryError::PublicUnavailable)
    }

    pub(crate) fn check_world(&self, world: WorldId) -> Result<()> {
        self.domain.check_world(world)
    }

    fn check_owner(&self, owner: InventoryOwner) -> Result<()> {
        if owner.domain() == self.domain {
            Ok(())
        } else {
            Err(InventoryError::WrongDomain)
        }
    }

    // Domain precedes slot existence, occupation, and generation validation.
    fn entry(&self, handle: InventoryHandle) -> Result<&Entry> {
        if handle.domain() != self.domain {
            return Err(InventoryError::WrongDomain);
        }
        self.slots
            .get(handle.slot() as usize)
            .filter(|slot| slot.generation == handle.generation())
            .and_then(|slot| slot.entry.as_ref())
            .ok_or(InventoryError::InvalidInventory)
    }

    pub(crate) fn owner(&self, handle: InventoryHandle) -> Result<InventoryOwner> {
        Ok(InventoryOwner::new(self.domain, self.entry(handle)?.body))
    }

    pub(crate) fn create(&mut self, owner: InventoryOwner) -> Result<InventoryHandle> {
        self.check_owner(owner)?;
        if let Some(public) = self.public
            && self.entry(public)?.body == owner.body()
        {
            return Err(InventoryError::PublicProtected);
        }
        let index = if let Some(&index) = self.free.last() {
            index
        } else {
            u32::try_from(self.slots.len()).map_err(|_| InventoryError::IdentityExhausted)?
        };
        // No recoverable errors remain beyond this point.
        if self.free.pop().is_none() {
            self.slots.push(Slot {
                generation: 1,
                entry: None,
            });
        }
        let slot = &mut self.slots[index as usize];
        debug_assert!(slot.entry.is_none());
        let handle = InventoryHandle::new(self.domain, index, slot.generation);
        slot.entry = Some(Entry {
            body: owner.body(),
            items: HashSet::new(),
        });
        self.owners.entry(owner.body()).or_default().insert(handle);
        self.live_count += 1;
        Ok(handle)
    }

    pub(crate) fn remove(&mut self, handle: InventoryHandle) -> Result<()> {
        let body = self.entry(handle)?.body;
        if self.public == Some(handle) {
            return Err(InventoryError::PublicProtected);
        }
        self.remove_validated(handle);
        let bucket = self
            .owners
            .get_mut(&body)
            .expect("occupied inventory has owner index");
        bucket.remove(&handle);
        if bucket.is_empty() {
            self.owners.remove(&body);
        }
        Ok(())
    }

    // Caller validated the identity (or took its exact owner bucket). Does not
    // touch the owner index, allowing clear_owner to take that bucket once.
    fn remove_validated(&mut self, handle: InventoryHandle) {
        let slot = &mut self.slots[handle.slot() as usize];
        debug_assert_eq!(slot.generation, handle.generation());
        let entry = slot.entry.take().expect("validated inventory is occupied");
        for item in entry.items {
            if let Some(public) = self.public {
                self.slots[public.slot() as usize]
                    .entry
                    .as_mut()
                    .expect("Public outlives ordinary inventories")
                    .items
                    .insert(item);
                self.holders.insert(item, public);
            } else {
                // Uninstalled standalone cores exist only at the private test seam.
                self.holders.remove(&item);
            }
        }
        let slot = &mut self.slots[handle.slot() as usize];
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(handle.slot());
        }
        // MAX is permanently retired; removal still succeeds.
        self.live_count -= 1;
    }

    pub(crate) fn insert(&mut self, handle: InventoryHandle, item: ItemId) -> Result<()> {
        self.entry(handle)?;
        if self.holders.contains_key(&item) {
            return Err(InventoryError::ItemAlreadyHeld);
        }
        self.slots[handle.slot() as usize]
            .entry
            .as_mut()
            .unwrap()
            .items
            .insert(item);
        self.holders.insert(item, handle);
        Ok(())
    }

    pub(crate) fn take(&mut self, handle: InventoryHandle, item: ItemId) -> Result<()> {
        self.entry(handle)?;
        if self.holders.get(&item) != Some(&handle) {
            return Err(InventoryError::ItemNotHeld);
        }
        if let Some(public) = self.public {
            return self.transfer(handle, public, item);
        }
        self.slots[handle.slot() as usize]
            .entry
            .as_mut()
            .unwrap()
            .items
            .remove(&item);
        self.holders.remove(&item);
        Ok(())
    }

    /// Removes terminal custody only; the caller coordinates item destruction.
    pub(crate) fn remove_destroyed_item(
        &mut self,
        handle: InventoryHandle,
        item: ItemId,
    ) -> Result<()> {
        self.entry(handle)?;
        if self.holders.get(&item) != Some(&handle) {
            return Err(InventoryError::ItemNotHeld);
        }
        self.slots[handle.slot() as usize]
            .entry
            .as_mut()
            .expect("validated inventory is occupied")
            .items
            .remove(&item);
        self.holders.remove(&item);
        Ok(())
    }

    /// Validates source, target, then custody even when source equals target.
    /// Atomicity covers only this core's sets/index, not gameplay side effects.
    pub(crate) fn transfer(
        &mut self,
        source: InventoryHandle,
        target: InventoryHandle,
        item: ItemId,
    ) -> Result<()> {
        self.entry(source)?;
        self.entry(target)?;
        if self.holders.get(&item) != Some(&source) {
            return Err(InventoryError::ItemNotHeld);
        }
        if source == target {
            return Ok(());
        }
        self.slots[source.slot() as usize]
            .entry
            .as_mut()
            .unwrap()
            .items
            .remove(&item);
        self.slots[target.slot() as usize]
            .entry
            .as_mut()
            .unwrap()
            .items
            .insert(item);
        self.holders.insert(item, target);
        Ok(())
    }

    /// Owned, unordered snapshot; it does not follow subsequent mutations.
    pub(crate) fn items(&self, handle: InventoryHandle) -> Result<Vec<ItemId>> {
        Ok(self.entry(handle)?.items.iter().copied().collect())
    }

    pub(crate) fn item_count(&self, handle: InventoryHandle) -> Result<usize> {
        Ok(self.entry(handle)?.items.len())
    }

    pub(crate) fn all(&self) -> Vec<InventoryHandle> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.entry
                    .as_ref()
                    .map(|_| InventoryHandle::new(self.domain, index as u32, slot.generation))
            })
            .collect()
    }

    /// Indexed custody lookup only; Life liveness is the adapter's concern.
    pub(crate) fn holder(&self, item: ItemId) -> Result<Option<InventoryHandle>> {
        Ok(self.holders.get(&item).copied())
    }

    pub(crate) fn for_body(&self, owner: InventoryOwner) -> Result<Vec<InventoryHandle>> {
        self.check_owner(owner)?;
        Ok(self
            .owners
            .get(&owner.body())
            .map(|handles| handles.iter().copied().collect())
            .unwrap_or_default())
    }

    /// Retires only this exact Body's inventories, spilling their direct items
    /// into Public. No recursive item/Body traversal or unrelated scan occurs.
    pub(crate) fn clear_owner(&mut self, body: ConceptBodyHandle) -> usize {
        if self
            .public
            .is_some_and(|handle| self.entry(handle).is_ok_and(|e| e.body == body))
        {
            return 0;
        }
        let Some(handles) = self.owners.remove(&body) else {
            return 0;
        };
        let count = handles.len();
        for handle in handles {
            self.remove_validated(handle);
        }
        count
    }

    pub(crate) fn len(&self) -> usize {
        self.live_count
    }
}
