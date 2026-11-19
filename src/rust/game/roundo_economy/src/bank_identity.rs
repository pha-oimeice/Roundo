use crate::BankError;
use bevy::ecs::world::WorldId;
use roundo_life::ConceptBodyHandle;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct BankDomain {
    world: WorldId,
    epoch: u64,
}

fn allocate_epoch(counter: &AtomicU64) -> Result<u64, BankError> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| BankError::IdentityExhausted)
}

impl BankDomain {
    pub(crate) fn try_new(world: WorldId) -> Result<Self, BankError> {
        Ok(Self {
            world,
            epoch: allocate_epoch(&NEXT_EPOCH)?,
        })
    }

    pub(crate) fn check_world(self, world: WorldId) -> Result<(), BankError> {
        if self.world == world {
            Ok(())
        } else {
            Err(BankError::WrongDomain)
        }
    }
}

/// Opaque bank identity, bound to a World and a never-reused resource epoch.
/// This version allocates banks monotonically; it does not reuse bank slots.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BankHandle {
    domain: BankDomain,
    slot: u64,
    generation: u64,
}
impl BankHandle {
    pub(crate) const fn new(domain: BankDomain, slot: u64, generation: u64) -> Self {
        Self {
            domain,
            slot,
            generation,
        }
    }
    pub(crate) const fn domain(self) -> BankDomain {
        self.domain
    }
    pub(crate) const fn slot(self) -> u64 {
        self.slot
    }
    pub(crate) const fn generation(self) -> u64 {
        self.generation
    }
}

/// Exact domain-bound Body selection, not proof of Life liveness or authorization.
/// Bound through [`crate::Banks::bind_owner`]; normal access rechecks exact liveness.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BankOwner {
    domain: BankDomain,
    body: ConceptBodyHandle,
}
impl BankOwner {
    pub(crate) const fn new(domain: BankDomain, body: ConceptBodyHandle) -> Self {
        Self { domain, body }
    }
    pub(crate) const fn domain(self) -> BankDomain {
        self.domain
    }
    pub(crate) const fn body(self) -> ConceptBodyHandle {
        self.body
    }
}
