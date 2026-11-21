//! Bounded lifecycle/typed structure requests; only the commit kernel borrows World.
use crate::{
    ConceptBodyFactory, ConceptBodyHandle, ConceptBodyPrototype, FactoryError, LifeAdministration,
    LifeFactory, LifeHandle, LifePrototype, with_life_administration,
};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::*,
};
use std::collections::{HashMap, VecDeque};

/// Maximum scheduled Factory/structure pending + in-flight + unclaimed requests
/// plus the one reserved live host-scope slot, combined per App. Results are
/// never silently evicted: callers must consume them before submitting more work.
pub const FACTORY_REQUEST_CAPACITY: usize = 256;

/// App-affine correlation identity, not an instance Handle or lifecycle grant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FactoryRequestId {
    world: WorldId,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactorySubmitError {
    CapacityExceeded,
    IdExhausted,
}

/// Published only after Factory validation/materialization or removal completes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryCompletion {
    LifeCreated(LifeHandle),
    BodyCreated(ConceptBodyHandle),
    Removed,
    Rejected(FactoryError),
    /// Recorded before resuming the original unwind. Creation compensates its
    /// owned entity/slot; removal may already have invalidated the instance when
    /// an observer panics. Do not treat this as success or automatically retry.
    Panicked,
}

#[derive(Clone, Copy)]
pub(crate) enum ManagementCompletion {
    Factory(FactoryCompletion),
    Structure(crate::structure::LifeStructureCompletion),
}

pub(crate) trait ManagementOperation: Send + Sync {
    fn panicked(&self) -> ManagementCompletion;
    fn commit(
        self: Box<Self>,
        world: &mut World,
        admin: &LifeAdministration<'_>,
    ) -> ManagementCompletion;
}

fn factory_completion(result: Result<FactoryCompletion, FactoryError>) -> ManagementCompletion {
    ManagementCompletion::Factory(result.unwrap_or_else(FactoryCompletion::Rejected))
}
pub(crate) struct CreateLife(pub(crate) LifePrototype);
impl ManagementOperation for CreateLife {
    fn panicked(&self) -> ManagementCompletion {
        ManagementCompletion::Factory(FactoryCompletion::Panicked)
    }
    fn commit(
        self: Box<Self>,
        world: &mut World,
        admin: &LifeAdministration<'_>,
    ) -> ManagementCompletion {
        factory_completion(
            LifeFactory::create(world, admin, &self.0).map(FactoryCompletion::LifeCreated),
        )
    }
}
pub(crate) struct CreateBody<B: Bundle>(pub(crate) ConceptBodyPrototype<B>);
impl<B: Bundle> ManagementOperation for CreateBody<B> {
    fn panicked(&self) -> ManagementCompletion {
        ManagementCompletion::Factory(FactoryCompletion::Panicked)
    }
    fn commit(
        self: Box<Self>,
        world: &mut World,
        admin: &LifeAdministration<'_>,
    ) -> ManagementCompletion {
        factory_completion(
            ConceptBodyFactory::create(world, admin, self.0).map(FactoryCompletion::BodyCreated),
        )
    }
}
pub(crate) enum RemoveInstance {
    Life(LifeHandle),
    Body(ConceptBodyHandle),
}
impl ManagementOperation for RemoveInstance {
    fn panicked(&self) -> ManagementCompletion {
        ManagementCompletion::Factory(FactoryCompletion::Panicked)
    }
    fn commit(
        self: Box<Self>,
        world: &mut World,
        admin: &LifeAdministration<'_>,
    ) -> ManagementCompletion {
        let result = match *self {
            Self::Life(handle) => LifeFactory::remove(world, admin, handle),
            Self::Body(handle) => ConceptBodyFactory::remove(world, admin, handle),
        };
        factory_completion(result.map(|()| FactoryCompletion::Removed))
    }
}

#[derive(Resource)]
pub(crate) struct FactoryRequests {
    world: WorldId,
    next_sequence: u64,
    in_flight: usize,
    pending: VecDeque<(FactoryRequestId, Box<dyn ManagementOperation>)>,
    completed: HashMap<FactoryRequestId, ManagementCompletion>,
    pub(crate) scoped: Option<crate::scoped_management::ScopedRequests>,
}
impl FromWorld for FactoryRequests {
    fn from_world(world: &mut World) -> Self {
        Self {
            world: world.id(),
            next_sequence: 1,
            in_flight: 0,
            pending: VecDeque::new(),
            completed: HashMap::new(),
            scoped: None,
        }
    }
}

/// Trusted control-plane Interface. Body data Traits receive typed readers and
/// writers, not this parameter. Possession of a Handle alone grants no removal.
/// Enqueueing is NOT success and does not publish a Handle. Order request systems
/// before `LifeFixedSet::CommitFactories`, and result consumers after it. Requests
/// submitted after that point commit next tick. This is lifecycle-only: no caller
/// callback, arbitrary Commands, registration access or World is exposed.
#[derive(SystemParam)]
pub struct FactoryManagement<'w> {
    requests: ResMut<'w, FactoryRequests>,
}
impl FactoryRequests {
    pub(crate) fn submit(
        &mut self,
        operation: impl ManagementOperation + 'static,
    ) -> Result<FactoryRequestId, FactorySubmitError> {
        if self.occupied() >= FACTORY_REQUEST_CAPACITY {
            return Err(FactorySubmitError::CapacityExceeded);
        }
        let id = self.allocate_id()?;
        self.pending.push_back((id, Box::new(operation)));
        Ok(id)
    }

    // A scoped host reserves one slot across all its phases, including idle
    // barriers, so observer submissions cannot steal compensation capacity.
    pub(crate) fn occupied(&self) -> usize {
        self.pending.len()
            + self.completed.len()
            + self.in_flight
            + usize::from(self.scoped.is_some())
    }

    pub(crate) fn allocate_id(&mut self) -> Result<FactoryRequestId, FactorySubmitError> {
        let sequence = self.next_sequence;
        self.next_sequence = sequence
            .checked_add(1)
            .ok_or(FactorySubmitError::IdExhausted)?;
        Ok(FactoryRequestId {
            world: self.world,
            sequence,
        })
    }

    pub(crate) fn take_completion(&mut self, id: FactoryRequestId) -> Option<ManagementCompletion> {
        if id.world != self.world {
            return None;
        }
        self.completed.remove(&id)
    }
}

impl FactoryManagement<'_> {
    pub fn create_life(
        &mut self,
        prototype: LifePrototype,
    ) -> Result<FactoryRequestId, FactorySubmitError> {
        self.requests.submit(CreateLife(prototype))
    }
    pub fn create_body<B: Bundle>(
        &mut self,
        prototype: ConceptBodyPrototype<B>,
    ) -> Result<FactoryRequestId, FactorySubmitError> {
        self.requests.submit(CreateBody(prototype))
    }
    pub fn remove_life(
        &mut self,
        handle: LifeHandle,
    ) -> Result<FactoryRequestId, FactorySubmitError> {
        self.requests.submit(RemoveInstance::Life(handle))
    }
    pub fn remove_body(
        &mut self,
        handle: ConceptBodyHandle,
    ) -> Result<FactoryRequestId, FactorySubmitError> {
        self.requests.submit(RemoveInstance::Body(handle))
    }
    /// Consumes a completion exactly once. Pending, unknown, consumed and foreign
    /// App IDs return None; retaining the ID never retains instance validity.
    pub fn take_completion(&mut self, id: FactoryRequestId) -> Option<FactoryCompletion> {
        match self.requests.take_completion(id)? {
            ManagementCompletion::Factory(completion) => Some(completion),
            ManagementCompletion::Structure(_) => unreachable!("Factory IDs are not structure IDs"),
        }
    }
}

/// The sole scheduled lifecycle/typed structural commit. Processes the FIFO present at
/// entry, so observer-submitted work cannot make this tick's batch unbounded.
pub(crate) fn commit_factory_requests(world: &mut World) {
    let batch_len = world.resource::<FactoryRequests>().pending.len();
    for _ in 0..batch_len {
        let Some((id, operation)) = world.resource_mut::<FactoryRequests>().pending.pop_front()
        else {
            break;
        };
        world.resource_mut::<FactoryRequests>().in_flight += 1;
        let (completion, unwind) = execute_operation(world, operation);
        let mut requests = world.resource_mut::<FactoryRequests>();
        requests.in_flight -= 1;
        requests.completed.insert(id, completion);
        drop(requests);
        if let Some(payload) = unwind {
            std::panic::resume_unwind(payload);
        }
    }
}

/// Shared closed structural kernel. Both commit protocols record the returned
/// panic completion before resuming the original payload; neither grants World
/// to the request producer or result consumer.
pub(crate) fn execute_operation(
    world: &mut World,
    operation: Box<dyn ManagementOperation>,
) -> (ManagementCompletion, Option<Box<dyn std::any::Any + Send>>) {
    let panicked = operation.panicked();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_life_administration(world, |world, admin| operation.commit(world, admin))
    })) {
        Ok(completion) => (completion, None),
        Err(payload) => (panicked, Some(payload)),
    }
}
