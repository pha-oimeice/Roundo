//! Typed structure requests for existing instances, without exposing World or Commands.
use crate::{
    ConceptBodyHandle, EventData, EventError, EventPolicy, FactoryRequestId, FactorySubmitError,
    LifeAdministration, LifeHandle, StateData, StateError, StatePolicy,
    management::{FactoryRequests, ManagementCompletion, ManagementOperation},
};
use bevy::{
    ecs::system::SystemParam,
    prelude::{ResMut, World},
};
use std::marker::PhantomData;

/// App-affine structure correlation ID. Neither an instance Handle nor a Factory ID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LifeStructureRequestId(FactoryRequestId);

/// The typed kernel result, published after structural mutation and reverse cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifeStructureCompletion {
    State(Result<(), StateError>),
    Event(Result<(), EventError>),
    /// Recorded before resuming the original unwind. This is not a transactional
    /// rollback guarantee for arbitrary component hooks or payload destructors.
    Panicked,
}

pub(crate) enum StateOperation<S: StateData> {
    DeclareWriter(ConceptBodyHandle),
    DeclareReader(ConceptBodyHandle),
    InstallPolicy(LifeHandle, StatePolicy),
    CreateSlot(LifeHandle, ConceptBodyHandle, Option<S>),
    RemoveSlot(LifeHandle),
    RemoveSlotIfGeneration(LifeHandle, u64),
}
impl<S: StateData> ManagementOperation for StateOperation<S> {
    fn panicked(&self) -> ManagementCompletion {
        ManagementCompletion::Structure(LifeStructureCompletion::Panicked)
    }
    fn commit(
        self: Box<Self>,
        world: &mut World,
        admin: &LifeAdministration<'_>,
    ) -> ManagementCompletion {
        let result = match *self {
            Self::DeclareWriter(body) => crate::declare_state_writer::<S>(world, admin, body),
            Self::DeclareReader(body) => crate::declare_state_reader::<S>(world, admin, body),
            Self::InstallPolicy(life, policy) => {
                crate::install_state_policy::<S>(world, admin, life, policy)
            }
            Self::CreateSlot(life, origin, initial) => {
                crate::create_state_slot(world, admin, life, origin, initial)
            }
            Self::RemoveSlot(life) => crate::remove_state_slot::<S>(world, admin, life),
            Self::RemoveSlotIfGeneration(life, generation) => {
                crate::state::remove_state_slot_if_generation::<S>(world, admin, life, generation)
            }
        };
        ManagementCompletion::Structure(LifeStructureCompletion::State(result))
    }
}

enum EventOperation<E: EventData> {
    DeclareWriter(ConceptBodyHandle, PhantomData<E>),
    DeclareReader(ConceptBodyHandle, PhantomData<E>),
    InstallInterface(LifeHandle, EventPolicy, PhantomData<E>),
    RemoveInterface(LifeHandle, PhantomData<E>),
}
impl<E: EventData> ManagementOperation for EventOperation<E> {
    fn panicked(&self) -> ManagementCompletion {
        ManagementCompletion::Structure(LifeStructureCompletion::Panicked)
    }
    fn commit(
        self: Box<Self>,
        world: &mut World,
        admin: &LifeAdministration<'_>,
    ) -> ManagementCompletion {
        let result = match *self {
            Self::DeclareWriter(body, _) => crate::declare_event_writer::<E>(world, admin, body),
            Self::DeclareReader(body, _) => crate::declare_event_reader::<E>(world, admin, body),
            Self::InstallInterface(life, policy, _) => {
                crate::install_event_interface::<E>(world, admin, life, policy)
            }
            Self::RemoveInterface(life, _) => {
                crate::remove_event_interface::<E>(world, admin, life)
            }
        };
        ManagementCompletion::Structure(LifeStructureCompletion::Event(result))
    }
}

/// Trusted typed structural control-plane Interface; no standalone approval,
/// registration, data access, callback or arbitrary Commands capability. The existing typed
/// catalogue/declaration/policy checks run at commit, not at enqueue.
///
/// Shares the Factory FIFO and `FACTORY_REQUEST_CAPACITY` bound (pending +
/// in-flight + unclaimed results). Submit before `LifeFixedSet::CommitFactories`
/// and consume after it; late/observer requests wait for the next batch. Use
/// `ParamSet` when also borrowing `FactoryManagement` in the same system.
///
/// Declaration installation is idempotent; policy/interface/Slot installation
/// rejects duplicates. Slot creation requires prior native origin approval;
/// enqueueing the request neither grants approval nor makes the Slot visible.
/// Successful Slot creation registers its approved immutable origin, as in the
/// synchronous kernel. Removal cleans reverse registrations before its result. This
/// Interface deliberately adds no declaration/policy revocation semantics.
#[derive(SystemParam)]
pub struct LifeStructureManagement<'w> {
    requests: ResMut<'w, FactoryRequests>,
}
impl LifeStructureManagement<'_> {
    fn submit(
        &mut self,
        operation: impl ManagementOperation + 'static,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.requests.submit(operation).map(LifeStructureRequestId)
    }
    pub fn declare_state_writer<S: StateData>(
        &mut self,
        body: ConceptBodyHandle,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(StateOperation::<S>::DeclareWriter(body))
    }
    pub fn declare_state_reader<S: StateData>(
        &mut self,
        body: ConceptBodyHandle,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(StateOperation::<S>::DeclareReader(body))
    }
    pub fn install_state_policy<S: StateData>(
        &mut self,
        life: LifeHandle,
        policy: StatePolicy,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(StateOperation::<S>::InstallPolicy(life, policy))
    }
    pub fn create_state_slot<S: StateData>(
        &mut self,
        life: LifeHandle,
        origin: ConceptBodyHandle,
        initial: Option<S>,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(StateOperation::CreateSlot(life, origin, initial))
    }
    pub fn remove_state_slot<S: StateData>(
        &mut self,
        life: LifeHandle,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(StateOperation::<S>::RemoveSlot(life))
    }
    /// Conditional structural removal: compare the stable Slot generation at
    /// commit, not enqueue. A replacement returns SlotChanged without mutation.
    pub fn remove_state_slot_if_generation<S: StateData>(
        &mut self,
        life: LifeHandle,
        generation: u64,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(StateOperation::<S>::RemoveSlotIfGeneration(
            life, generation,
        ))
    }
    pub fn declare_event_writer<E: EventData>(
        &mut self,
        body: ConceptBodyHandle,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(EventOperation::<E>::DeclareWriter(body, PhantomData))
    }
    pub fn declare_event_reader<E: EventData>(
        &mut self,
        body: ConceptBodyHandle,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(EventOperation::<E>::DeclareReader(body, PhantomData))
    }
    pub fn install_event_interface<E: EventData>(
        &mut self,
        life: LifeHandle,
        policy: EventPolicy,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(EventOperation::<E>::InstallInterface(
            life,
            policy,
            PhantomData,
        ))
    }
    pub fn remove_event_interface<E: EventData>(
        &mut self,
        life: LifeHandle,
    ) -> Result<LifeStructureRequestId, FactorySubmitError> {
        self.submit(EventOperation::<E>::RemoveInterface(life, PhantomData))
    }
    /// Consumes the result once. Pending, unknown, consumed and foreign App IDs
    /// return None. Keeping this ID does not keep an instance or registration live.
    pub fn take_completion(
        &mut self,
        id: LifeStructureRequestId,
    ) -> Option<LifeStructureCompletion> {
        match self.requests.take_completion(id.0)? {
            ManagementCompletion::Structure(completion) => Some(completion),
            ManagementCompletion::Factory(_) => unreachable!("Structure IDs are not Factory IDs"),
        }
    }
}
