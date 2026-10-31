//! Controller authority gate shared by live Player and server-local AI admission.
//! One value governs one Controller; composition owns FIFO settlement and liveness.
//! This module neither owns Intent Body data nor authenticates network envelopes.

use crate::AiControllerId;
use roundo_contracts::{ConnectionId, ControllerAccessLevel, ControllerBindingId, PlayerId};
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

/// Server-admitted connection identity. The generation changes on replacement,
/// including when the transport reuses the same ConnectionId.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmittedConnection {
    pub id: ConnectionId,
    pub generation: u64,
}

/// Authenticated ingress, not a caller-supplied wire field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Player {
        player: PlayerId,
        connection: AdmittedConnection,
    },
    LocalAi(AiControllerId),
}

/// Supplied by composition at the immediate authorization point. Implementations
/// must verify server-authenticated admission AND principal association, or local
/// registration/liveness; a client cannot assert any of these facts.
pub trait AuthorityFacts {
    fn player_access(&self, player: PlayerId) -> ControllerAccessLevel;
    /// Monotonic revocation stamp, including downgrades already restored before this call.
    fn player_access_revision(&self, player: PlayerId) -> Option<u64>;
    /// True only when this is the sole admitted producer for the Player.
    /// Admission/quarantine must be serialized with gate transitions.
    fn sole_admitted_for(&self, player: PlayerId, connection: AdmittedConnection) -> bool;
    fn live_ai(&self, ai: AiControllerId) -> bool;
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

impl From<BindingId> for ControllerBindingId {
    fn from(binding: BindingId) -> Self {
        Self(binding.0)
    }
}

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
    source: Source,
    id: BindingId,
    access_revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Transition {
    request: RequestId,
    source: Source,
    access_revision: Option<u64>,
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
    pub fn belongs_to(&self, source: Source) -> bool {
        self.binding.is_some_and(|b| b.source == source)
            || self.transition.is_some_and(|t| t.source == source)
    }
    pub fn belongs_to_player(&self, player: PlayerId) -> bool {
        self.binding
            .is_some_and(|b| matches!(b.source, Source::Player { player: p, .. } if p == player))
            || self.transition.is_some_and(
                |t| matches!(t.source, Source::Player { player: p, .. } if p == player),
            )
    }
    pub fn state(&self) -> GateState {
        self.state
    }
    pub fn active_source(&self) -> Option<Source> {
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
    pub fn draining_binding(&self, request: RequestId, source: Source) -> Option<BindingId> {
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

    fn revision(source: Source, facts: &impl AuthorityFacts) -> Option<u64> {
        match source {
            Source::Player { player, .. } => facts.player_access_revision(player),
            Source::LocalAi(_) => Some(0),
        }
    }

    fn authorize(source: Source, facts: &impl AuthorityFacts) -> Result<(), GateError> {
        match source {
            Source::Player { player, connection }
                if facts.player_access(player) == ControllerAccessLevel::ReadWrite
                    && facts.player_access_revision(player).is_some()
                    && facts.sole_admitted_for(player, connection) =>
            {
                Ok(())
            }
            Source::LocalAi(ai) if facts.live_ai(ai) => Ok(()),
            _ => Err(GateError::Unauthorized),
        }
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
        source: Source,
        facts: &impl AuthorityFacts,
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
        Self::authorize(source, facts)?;
        self.valid_deadline(time)?;
        let request = RequestId(allocate_authority_id()?);
        self.transition = Some(Transition {
            request,
            source,
            access_revision: Self::revision(source, facts),
            deadline: time.deadline,
            proof: false,
        });
        self.state = GateState::Preparing;
        Ok(request)
    }

    /// Ready must be received through the same authenticated source after the
    /// client discarded its old pending input/prediction; AI Ready is local.
    pub fn commit_acquire(
        &mut self,
        request: RequestId,
        source: Source,
        facts: &impl AuthorityFacts,
        now: u64,
        ready: bool,
    ) -> Result<BindingId, GateError> {
        self.expire_if_due(now);
        let transition = self.check_transition(GateState::Preparing, request, source, now)?;
        if !ready {
            return Err(GateError::NotReady);
        }
        Self::authorize(source, facts)?;
        if Self::revision(source, facts) != transition.access_revision {
            self.revoke();
            return Err(GateError::Unauthorized);
        }
        let id = BindingId(allocate_authority_id()?);
        self.binding = Some(Binding {
            source: transition.source,
            id,
            access_revision: transition.access_revision,
        });
        self.finish(request, TerminalOutcome::Acquired(id), GateState::Active);
        Ok(id)
    }

    /// Closes Submit synchronously, before any producer drain is initiated.
    pub fn begin_release(
        &mut self,
        source: Source,
        facts: &impl AuthorityFacts,
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
        if Self::authorize(source, facts).is_err()
            || Self::revision(source, facts) != self.binding.unwrap().access_revision
        {
            self.revoke();
            return Err(GateError::Unauthorized);
        }
        self.valid_deadline(time)?;
        let request = RequestId(allocate_authority_id()?);
        self.transition = Some(Transition {
            request,
            source,
            access_revision: self.binding.unwrap().access_revision,
            deadline: time.deadline,
            proof: false,
        });
        self.state = GateState::Draining;
        Ok(request)
    }

    /// Proof is the ECS-observed stream-0 prefix of the exact old binding.
    pub fn record_prefix(
        &mut self,
        request: RequestId,
        connection: AdmittedConnection,
        now: u64,
    ) -> Result<(), GateError> {
        self.expire_if_due(now);
        let source = self.transition.ok_or(GateError::WrongState)?.source;
        self.check_transition(GateState::Draining, request, source, now)?;
        if !matches!(source, Source::Player { connection: expected, .. } if expected == connection)
        {
            return Err(GateError::WrongSource);
        }
        self.transition.as_mut().unwrap().proof = true;
        Ok(())
    }

    /// Local AI producers synchronously stop and confirm revocation, not a wire prefix.
    pub fn record_local_revocation(
        &mut self,
        request: RequestId,
        ai: AiControllerId,
        now: u64,
    ) -> Result<(), GateError> {
        self.expire_if_due(now);
        self.check_transition(GateState::Draining, request, Source::LocalAi(ai), now)?;
        self.transition.as_mut().unwrap().proof = true;
        Ok(())
    }

    /// `pending_old_work_settled` must be established by composition after the
    /// old tick's movement/gaze is committed and held motion is zeroed.
    pub fn commit_release(
        &mut self,
        request: RequestId,
        source: Source,
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

    /// Composition calls this after invalidating a Controller, disconnect, access
    /// revocation, or AI removal. It cannot reopen a quarantined owner.
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
        source: Source,
        facts: &impl AuthorityFacts,
    ) -> Result<BindingId, GateError> {
        if self.state != GateState::Active {
            return Err(GateError::WrongState);
        }
        let binding = self.binding.ok_or(GateError::WrongState)?;
        if binding.source != source {
            return Err(GateError::WrongSource);
        }
        Self::authorize(source, facts)?;
        if Self::revision(source, facts) != binding.access_revision {
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
        source: Source,
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
