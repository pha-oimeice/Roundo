use bevy::ecs::world::WorldId;
use roundo_life::ConceptBodyHandle;
use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

/// An externally supplied identity, independent of item type or associated data.
/// Importing or copying an ID does not issue a new item. The caller is responsible
/// for globally unique, never-reused identities across authoritative domains.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemId(u128);

impl ItemId {
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

/// World affinity plus a resource incarnation; neither is an item ID issuer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InventoryDomain {
    world: WorldId,
    epoch: u64,
}

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

fn allocate_epoch(counter: &AtomicU64) -> Result<u64, InventoryError> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| InventoryError::IdentityExhausted)
}

impl InventoryDomain {
    pub(crate) fn try_new(world: WorldId) -> Result<Self, InventoryError> {
        Ok(Self {
            world,
            epoch: allocate_epoch(&NEXT_EPOCH)?,
        })
    }

    pub(crate) fn check_world(self, world: WorldId) -> Result<(), InventoryError> {
        if self.world == world {
            Ok(())
        } else {
            Err(InventoryError::WrongDomain)
        }
    }
}

/// Opaque, generational inventory identity bound to one resource incarnation.
/// An old identity cannot address a replacement inventory or reinstalled store.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InventoryHandle {
    domain: InventoryDomain,
    slot: u32,
    generation: u64,
}

impl InventoryHandle {
    pub(crate) const fn new(domain: InventoryDomain, slot: u32, generation: u64) -> Self {
        Self {
            domain,
            slot,
            generation,
        }
    }
    pub(crate) const fn domain(self) -> InventoryDomain {
        self.domain
    }
    pub(crate) const fn slot(self) -> u32 {
        self.slot
    }
    pub(crate) const fn generation(self) -> u64 {
        self.generation
    }
}

/// A domain-bound exact Body selection, not Player authorization or proof of
/// the origin of a bare Body handle. Life liveness requires adapter validation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InventoryOwner {
    domain: InventoryDomain,
    body: ConceptBodyHandle,
}

impl InventoryOwner {
    pub(crate) const fn new(domain: InventoryDomain, body: ConceptBodyHandle) -> Self {
        Self { domain, body }
    }
    pub(crate) const fn domain(self) -> InventoryDomain {
        self.domain
    }
    /// The exact associated Body, not proof of its current liveness or authority.
    /// Use `Inventories` operations to revalidate this owner before accessing custody.
    pub const fn body(self) -> ConceptBodyHandle {
        self.body
    }
}

/// Structural errors leave the live core unchanged; panic/abort/allocation
/// failure and external side effects are outside that rollback guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryError {
    WrongDomain,
    InvalidInventory,
    OwnerNotLive,
    ItemAlreadyHeld,
    ItemNotHeld,
    IdentityExhausted,
    /// The unique Public inventory/owner is infrastructure, not removable.
    PublicProtected,
    /// The host has bypassed the installed Public infrastructure.
    PublicUnavailable,
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongDomain => "inventory identity belongs to another domain",
            Self::InvalidInventory => "inventory identity is invalid or stale",
            Self::OwnerNotLive => "inventory owner is not live",
            Self::ItemAlreadyHeld => "item is already held",
            Self::ItemNotHeld => "item is not held by the source inventory",
            Self::IdentityExhausted => "inventory identity space is exhausted",
            Self::PublicProtected => "Public inventory cannot be removed or duplicated",
            Self::PublicUnavailable => "Public inventory is not installed",
        })
    }
}
impl std::error::Error for InventoryError {}
