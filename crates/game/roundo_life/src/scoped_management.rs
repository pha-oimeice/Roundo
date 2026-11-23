//! Explicit, single-step structural commits for a serialized synchronous host.
//! The scheduled Factory FIFO is never drained, reordered or advanced here.

use crate::{
    ConceptBodyHandle, ConceptBodyPrototype, FactoryCompletion, FactoryRequestId,
    FactorySubmitError, LifeHandle, LifePrototype, StateData, StateError,
    management::{
        CreateBody, CreateLife, FactoryRequests, ManagementCompletion, ManagementOperation,
        RemoveInstance, execute_operation,
    },
    structure::{LifeStructureCompletion, StateOperation},
};
use bevy::{
    ecs::system::SystemParam,
    prelude::{Bundle, ResMut, World},
};

/// App-affine host scope, not a Handle, registration grant or scheduled request.
/// Only one scope can be live per App. The host must finish it after consuming
/// every completion, including any Panicked result during compensation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LifeCommitScope(FactoryRequestId);

/// Correlation ID for one request in a live host scope. It cannot be supplied to
/// the scheduled Factory/structure commit or result Interfaces.
///
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScopedLifeRequestId {
    scope: LifeCommitScope,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopedLifeError {
    Submit(FactorySubmitError),
    ScopeBusy,
    InvalidScope,
    OutstandingRequest,
    UnknownRequest,
}
impl From<FactorySubmitError> for ScopedLifeError {
    fn from(value: FactorySubmitError) -> Self {
        Self::Submit(value)
    }
}

/// Real kernel outcome, available only after the explicitly selected commit.
/// The scope's single slot stays occupied until the result is consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopedLifeCompletion {
    Factory(FactoryCompletion),
    State(Result<(), StateError>),
    /// Recorded before rethrowing the original panic. Factory creation cleans
    /// its own partial entity/slot; removal may already be logically effective.
    Panicked,
}

enum ScopedPhase {
    Idle,
    Pending {
        id: ScopedLifeRequestId,
        operation: Box<dyn ManagementOperation>,
    },
    InFlight {
        id: ScopedLifeRequestId,
    },
    Completed {
        id: ScopedLifeRequestId,
        completion: ManagementCompletion,
    },
}

pub(crate) struct ScopedRequests {
    scope: LifeCommitScope,
    next_sequence: u64,
    phase: ScopedPhase,
}
impl ScopedRequests {
    fn occupied(&self) -> bool {
        !matches!(self.phase, ScopedPhase::Idle)
    }

    fn submit(
        &mut self,
        operation: impl ManagementOperation + 'static,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        if self.occupied() {
            return Err(ScopedLifeError::OutstandingRequest);
        }
        let sequence = self.next_sequence;
        self.next_sequence = sequence
            .checked_add(1)
            .ok_or(FactorySubmitError::IdExhausted)?;
        let id = ScopedLifeRequestId {
            scope: self.scope,
            sequence,
        };
        self.phase = ScopedPhase::Pending {
            id,
            operation: Box::new(operation),
        };
        Ok(id)
    }

    fn start_commit(
        &mut self,
        id: ScopedLifeRequestId,
    ) -> Result<Box<dyn ManagementOperation>, ScopedLifeError> {
        if !matches!(&self.phase, ScopedPhase::Pending { id: pending, .. } if *pending == id) {
            return Err(ScopedLifeError::UnknownRequest);
        }
        let ScopedPhase::Pending { operation, .. } =
            std::mem::replace(&mut self.phase, ScopedPhase::InFlight { id })
        else {
            unreachable!("checked pending request")
        };
        Ok(operation)
    }

    fn complete(&mut self, id: ScopedLifeRequestId, completion: ManagementCompletion) {
        assert!(
            matches!(&self.phase, ScopedPhase::InFlight { id: current } if *current == id),
            "only the exact in-flight request can complete"
        );
        self.phase = ScopedPhase::Completed { id, completion };
    }

    fn take_completion(&mut self, id: ScopedLifeRequestId) -> Option<ManagementCompletion> {
        if !matches!(&self.phase, ScopedPhase::Completed { id: current, .. } if *current == id) {
            return None;
        }
        let ScopedPhase::Completed { completion, .. } =
            std::mem::replace(&mut self.phase, ScopedPhase::Idle)
        else {
            unreachable!("checked completed request")
        };
        Some(completion)
    }

    fn cancel(&mut self, id: ScopedLifeRequestId) -> Result<(), ScopedLifeError> {
        if !matches!(&self.phase, ScopedPhase::Pending { id: pending, .. } if *pending == id) {
            return Err(ScopedLifeError::UnknownRequest);
        }
        // Remove the pending request before dropping its operation payload.
        let canceled = std::mem::replace(&mut self.phase, ScopedPhase::Idle);
        drop(canceled);
        Ok(())
    }
}

/// Native request/result Interface for an already serialized synchronous host,
/// not an alternative automatically scheduled FIFO. Only Factory create/remove
/// and typed State Slot creation/removal are supported; no callbacks, World, arbitrary
/// Commands, standalone approval/registration or data capability are exposed.
///
/// Beginning a scope reserves one of the App's 256 management slots for its
/// entire lifetime, including barriers between requests. Each scope permits one
/// pending/in-flight/unclaimed request at a time. This prevents ordinary or
/// observer submissions from consuming the slot needed for reverse compensation.
/// Enqueue is not success; the host calls `commit_scoped_life_request` for the
/// exact ID, and a native consumer takes the result before the next submission.
/// The scheduled queue keeps its original fixed-tick batch and FIFO contract.
#[derive(SystemParam)]
pub struct LifeScopedManagement<'w> {
    requests: ResMut<'w, FactoryRequests>,
}
impl LifeScopedManagement<'_> {
    pub fn begin_scope(&mut self) -> Result<LifeCommitScope, ScopedLifeError> {
        if self.requests.scoped.is_some() {
            return Err(ScopedLifeError::ScopeBusy);
        }
        if self.requests.occupied() >= crate::FACTORY_REQUEST_CAPACITY {
            return Err(FactorySubmitError::CapacityExceeded.into());
        }
        let scope = LifeCommitScope(self.requests.allocate_id()?);
        self.requests.scoped = Some(ScopedRequests {
            scope,
            next_sequence: 1,
            phase: ScopedPhase::Idle,
        });
        Ok(scope)
    }

    fn scoped(&mut self, scope: LifeCommitScope) -> Result<&mut ScopedRequests, ScopedLifeError> {
        self.requests
            .scoped
            .as_mut()
            .filter(|active| active.scope == scope)
            .ok_or(ScopedLifeError::InvalidScope)
    }

    fn submit(
        &mut self,
        scope: LifeCommitScope,
        operation: impl ManagementOperation + 'static,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.scoped(scope)?.submit(operation)
    }

    pub fn create_life(
        &mut self,
        scope: LifeCommitScope,
        prototype: LifePrototype,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(scope, CreateLife(prototype))
    }

    pub fn create_body<B: Bundle>(
        &mut self,
        scope: LifeCommitScope,
        prototype: ConceptBodyPrototype<B>,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(scope, CreateBody(prototype))
    }

    pub fn remove_life(
        &mut self,
        scope: LifeCommitScope,
        handle: LifeHandle,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(scope, RemoveInstance::Life(handle))
    }

    pub fn remove_body(
        &mut self,
        scope: LifeCommitScope,
        handle: ConceptBodyHandle,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(scope, RemoveInstance::Body(handle))
    }

    pub fn create_state_slot<S: StateData>(
        &mut self,
        scope: LifeCommitScope,
        life: LifeHandle,
        origin: ConceptBodyHandle,
        initial: Option<S>,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(scope, StateOperation::CreateSlot(life, origin, initial))
    }

    /// Removes only this typed Slot, never its Life, policies or participant
    /// Bodies. Enqueue is not removal: the same exact scoped commit and one-shot
    /// State completion protocol applies. The scope's reserved slot remains
    /// available even when scheduled requests fill the rest of the shared budget.
    pub fn remove_state_slot<S: StateData>(
        &mut self,
        scope: LifeCommitScope,
        life: LifeHandle,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(scope, StateOperation::<S>::RemoveSlot(life))
    }

    /// At commit, removes only the stable generation observed by this host.
    /// Replacement returns SlotChanged; generation possession grants no authority.
    pub fn remove_state_slot_if_generation<S: StateData>(
        &mut self,
        scope: LifeCommitScope,
        life: LifeHandle,
        generation: u64,
    ) -> Result<ScopedLifeRequestId, ScopedLifeError> {
        self.submit(
            scope,
            StateOperation::<S>::RemoveSlotIfGeneration(life, generation),
        )
    }

    pub fn take_completion(
        &mut self,
        scope: LifeCommitScope,
        id: ScopedLifeRequestId,
    ) -> Option<ScopedLifeCompletion> {
        let completion = self.scoped(scope).ok()?.take_completion(id)?;
        Some(match completion {
            ManagementCompletion::Factory(FactoryCompletion::Panicked)
            | ManagementCompletion::Structure(LifeStructureCompletion::Panicked) => {
                ScopedLifeCompletion::Panicked
            }
            ManagementCompletion::Factory(value) => ScopedLifeCompletion::Factory(value),
            ManagementCompletion::Structure(LifeStructureCompletion::State(value)) => {
                ScopedLifeCompletion::State(value)
            }
            ManagementCompletion::Structure(LifeStructureCompletion::Event(_)) => {
                unreachable!("scoped Interface does not submit Event structure operations")
            }
        })
    }

    /// Cancels only an uncommitted request. No completed result or materialized
    /// instance is silently discarded; arbitrary payload destructor side effects
    /// are not a rollback guarantee.
    pub fn cancel_request(
        &mut self,
        scope: LifeCommitScope,
        id: ScopedLifeRequestId,
    ) -> Result<(), ScopedLifeError> {
        self.scoped(scope)?.cancel(id)
    }

    pub fn finish_scope(&mut self, scope: LifeCommitScope) -> Result<(), ScopedLifeError> {
        if self.scoped(scope)?.occupied() {
            return Err(ScopedLifeError::OutstandingRequest);
        }
        self.requests.scoped = None;
        Ok(())
    }
}

/// The explicit structural kernel for one scoped request. It cannot flush the
/// scheduled FIFO or another request. Missing/foreign/stale IDs reject before
/// taking an operation. The same closed Factory/State kernel is used by both
/// commit protocols; invalidation cleanup is complete before result publication.
/// The caller owns sequencing and must consume Panicked before compensating and
/// resuming its outer unwind. This entry never advances Life time.
pub fn commit_scoped_life_request(
    world: &mut World,
    scope: LifeCommitScope,
    id: ScopedLifeRequestId,
) -> Result<(), ScopedLifeError> {
    let operation = {
        let mut requests = world
            .get_resource_mut::<FactoryRequests>()
            .ok_or(ScopedLifeError::InvalidScope)?;
        let active = requests
            .scoped
            .as_mut()
            .filter(|active| active.scope == scope)
            .ok_or(ScopedLifeError::InvalidScope)?;
        active.start_commit(id)?
    };
    let (completion, unwind) = execute_operation(world, operation);
    let mut requests = world.resource_mut::<FactoryRequests>();
    let active = requests
        .scoped
        .as_mut()
        .expect("in-flight scope cannot finish");
    active.complete(id, completion);
    drop(requests);
    if let Some(payload) = unwind {
        std::panic::resume_unwind(payload);
    }
    Ok(())
}
