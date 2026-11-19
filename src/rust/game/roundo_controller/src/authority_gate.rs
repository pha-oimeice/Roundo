//! World-local Controller authority and ordered AI settlement.

use crate::AiControllerId;
use std::sync::Mutex;

// One process-scoped namespace for requests and bindings. Serialized allocation
// preserves request high-water order even when different gates interleave.
static NEXT_AUTHORITY_ID: Mutex<u128> = Mutex::new(1);

fn allocate_authority_id() -> Result<u128, GateError> {
    let mut next_id = NEXT_AUTHORITY_ID.lock().unwrap_or_else(|e| e.into_inner());
    let following = next_id.checked_add(1).ok_or(GateError::IdExhausted)?;
    let id = *next_id;
    *next_id = following;
    Ok(id)
}

/// All times are from the same non-wrapping authoritative clock, not wall time.
/// `deadline` is exclusive and duration is bounded by `max_span`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline {
    pub now: u64,
    pub deadline: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RequestId(pub u128);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindingId(pub u128);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateState {
    Closed,
    Active,
    Draining,
    Preparing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateError {
    Unauthorized,
    Occupied,
    WrongState,
    WrongRequest,
    WrongSource,
    MissingProof,
    PendingWork,
    NotReady,
    Deadline,
    IdExhausted,
    TerminalPending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalOutcome {
    Released,
    Acquired(BindingId),
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Terminal {
    pub request: RequestId,
    pub outcome: TerminalOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Binding {
    source: AiControllerId,
    id: BindingId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Transition {
    request: RequestId,
    source: AiControllerId,
    deadline: u64,
    proof: bool,
}

/// A separate state-machine value, deliberately not attached to live Controllers.
/// IDs share a process-scoped checked allocator; at most one transition and one
/// terminal are retained. The caller serializes all gate operations at one ingress.
#[derive(Debug)]
pub struct AuthorityGate {
    state: GateState,
    binding: Option<Binding>,
    transition: Option<Transition>,
    terminal: Option<Terminal>,
    max_span: u64,
}

impl AuthorityGate {
    pub fn new(max_span: u64) -> Self {
        Self {
            state: GateState::Closed,
            binding: None,
            transition: None,
            terminal: None,
            max_span,
        }
    }

    /// Trusted composition revocation selector; never grants authority.
    pub fn belongs_to(&self, source: AiControllerId) -> bool {
        self.binding.is_some_and(|b| b.source == source)
            || self.transition.is_some_and(|t| t.source == source)
    }
    pub fn state(&self) -> GateState {
        self.state
    }
    pub fn active_source(&self) -> Option<AiControllerId> {
        (self.state == GateState::Active)
            .then(|| self.binding.map(|b| b.source))
            .flatten()
    }
    pub fn active_binding(&self) -> Option<BindingId> {
        (self.state == GateState::Active)
            .then(|| self.binding.map(|b| b.id))
            .flatten()
    }
    /// Read-only old binding identity for an exact in-flight Release.
    pub fn draining_binding(
        &self,
        request: RequestId,
        source: AiControllerId,
    ) -> Option<BindingId> {
        (self.state == GateState::Draining
            && self.transition.is_some_and(|transition| {
                transition.request == request && transition.source == source
            }))
        .then(|| {
            self.binding
                .filter(|binding| binding.source == source)
                .map(|binding| binding.id)
        })
        .flatten()
    }
    pub fn take_terminal(&mut self) -> Option<Terminal> {
        self.terminal.take()
    }

    fn valid_deadline(&self, time: Deadline) -> Result<(), GateError> {
        if time.deadline <= time.now || time.deadline - time.now > self.max_span {
            return Err(GateError::Deadline);
        }
        Ok(())
    }

    /// Begins only from unowned Closed, with no unconsumed terminal.
    pub fn begin_acquire(
        &mut self,
        source: AiControllerId,
        live: bool,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        if self.terminal.is_some() {
            return Err(GateError::TerminalPending);
        }
        if self.state != GateState::Closed {
            return Err(GateError::WrongState);
        }
        if self.binding.is_some() {
            return Err(GateError::Occupied);
        }
        if !live {
            return Err(GateError::Unauthorized);
        }
        self.valid_deadline(time)?;
        let request = RequestId(allocate_authority_id()?);
        self.transition = Some(Transition {
            request,
            source,
            deadline: time.deadline,
            proof: false,
        });
        self.state = GateState::Preparing;
        Ok(request)
    }

    pub fn commit_acquire(
        &mut self,
        request: RequestId,
        source: AiControllerId,
        live: bool,
        now: u64,
        ready: bool,
    ) -> Result<BindingId, GateError> {
        self.expire_if_due(now);
        let transition = self.check_transition(GateState::Preparing, request, source, now)?;
        if !ready {
            return Err(GateError::NotReady);
        }
        if !live {
            return Err(GateError::Unauthorized);
        }
        let id = BindingId(allocate_authority_id()?);
        self.binding = Some(Binding {
            source: transition.source,
            id,
        });
        self.finish(request, TerminalOutcome::Acquired(id), GateState::Active);
        Ok(id)
    }

    /// Closes Submit synchronously, before any producer drain is initiated.
    pub fn begin_release(
        &mut self,
        source: AiControllerId,
        live: bool,
        time: Deadline,
    ) -> Result<RequestId, GateError> {
        if self.terminal.is_some() {
            return Err(GateError::TerminalPending);
        }
        if self.state != GateState::Active {
            return Err(GateError::WrongState);
        }
        if self.binding.map(|b| b.source) != Some(source) {
            return Err(GateError::WrongSource);
        }
        if !live {
            self.revoke();
            return Err(GateError::Unauthorized);
        }
        self.valid_deadline(time)?;
        let request = RequestId(allocate_authority_id()?);
        self.transition = Some(Transition {
            request,
            source,
            deadline: time.deadline,
            proof: false,
        });
        self.state = GateState::Draining;
        Ok(request)
    }

    /// Records revocation after local input admission was closed.
    pub fn record_local_revocation(
        &mut self,
        request: RequestId,
        ai: AiControllerId,
        now: u64,
    ) -> Result<(), GateError> {
        self.expire_if_due(now);
        self.check_transition(GateState::Draining, request, ai, now)?;
        self.transition.as_mut().unwrap().proof = true;
        Ok(())
    }

    /// `pending_old_work_settled` must be established by composition after the
    /// old tick's movement/gaze is committed and held motion is zeroed.
    pub fn commit_release(
        &mut self,
        request: RequestId,
        source: AiControllerId,
        now: u64,
        pending_old_work_settled: bool,
    ) -> Result<(), GateError> {
        self.expire_if_due(now);
        let transition = self.check_transition(GateState::Draining, request, source, now)?;
        if !transition.proof {
            return Err(GateError::MissingProof);
        }
        if !pending_old_work_settled {
            return Err(GateError::PendingWork);
        }
        self.binding = None;
        self.finish(request, TerminalOutcome::Released, GateState::Closed);
        Ok(())
    }

    /// Explicit failure/expiry emits exactly one terminal; neither restores Active.
    /// A failed release keeps the old binding quarantined until explicit revocation.
    pub fn fail(&mut self, request: RequestId) -> Result<(), GateError> {
        let transition = self.transition.ok_or(GateError::WrongState)?;
        if transition.request != request {
            return Err(GateError::WrongRequest);
        }
        self.finish(request, TerminalOutcome::Failed, GateState::Closed);
        Ok(())
    }

    pub fn expire(&mut self, now: u64) -> Result<(), GateError> {
        let transition = self.transition.ok_or(GateError::WrongState)?;
        if now < transition.deadline {
            return Err(GateError::Deadline);
        }
        self.fail(transition.request)
    }

    pub fn revoke(&mut self) {
        if let Some(transition) = self.transition {
            self.finish(
                transition.request,
                TerminalOutcome::Failed,
                GateState::Closed,
            );
        }
        self.binding = None;
        self.state = GateState::Closed;
    }

    pub fn submit_if_active(
        &self,
        source: AiControllerId,
        live: bool,
    ) -> Result<BindingId, GateError> {
        if self.state != GateState::Active {
            return Err(GateError::WrongState);
        }
        let binding = self.binding.ok_or(GateError::WrongState)?;
        if binding.source != source {
            return Err(GateError::WrongSource);
        }
        if !live {
            return Err(GateError::Unauthorized);
        }
        Ok(binding.id)
    }

    fn expire_if_due(&mut self, now: u64) {
        if let Some(transition) = self.transition
            && now >= transition.deadline
        {
            self.finish(
                transition.request,
                TerminalOutcome::Failed,
                GateState::Closed,
            );
        }
    }

    fn check_transition(
        &self,
        state: GateState,
        request: RequestId,
        source: AiControllerId,
        now: u64,
    ) -> Result<Transition, GateError> {
        if self.state != state {
            return Err(GateError::WrongState);
        }
        let transition = self.transition.ok_or(GateError::WrongState)?;
        if transition.request != request {
            return Err(GateError::WrongRequest);
        }
        if transition.source != source {
            return Err(GateError::WrongSource);
        }
        if now >= transition.deadline {
            return Err(GateError::Deadline);
        }
        Ok(transition)
    }

    fn finish(&mut self, request: RequestId, outcome: TerminalOutcome, state: GateState) {
        self.transition = None;
        self.state = state;
        self.terminal = Some(Terminal { request, outcome });
    }
}
