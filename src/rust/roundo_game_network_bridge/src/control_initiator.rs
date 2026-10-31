//! Client-only request correlation. Only the production lifecycle may translate
//! a matched, Ready-sent terminal into a local controller binding.
use roundo_contracts::{
    ControllerId,
    control_handoff::{
        ControlDeadline, ControlHandoffError, ControlPreparing, ControlRequest, ControlRequestId,
        ControlRequestKind, ControlSuccess, ControlTerminal,
    },
};
use roundo_dsa::data_structure::async_tickets::{
    AsyncTickets, Completion, ReplyError, SubmitError, TicketId,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
const CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlInitiatorCompletion {
    Terminal(Result<ControlSuccess, ControlHandoffError>),
    DispatchFailed,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlInitiatorError {
    StaleSession,
    Full,
    DeadlineElapsed,
    ClockRegressed,
    IdExhausted,
    UnknownRequest,
    InProgress,
    Active,
    RetryRequired,
}

enum DispatchState {
    Awaiting,
    Reported,
}

struct RequestEntry {
    request: ControlRequest,
    ticket: TicketId,
    dispatch: DispatchState,
    preparing_seen: bool,
    ready_sent: bool,
    ticket_pending: bool,
}

pub(crate) struct ControlInitiatorBook {
    tickets: AsyncTickets<Result<ControlSuccess, ControlHandoffError>, ()>,
    requests: HashMap<ControlRequestId, RequestEntry>,
    ticket_requests: HashMap<TicketId, ControlRequestId>,
    now_value: u64,
}

impl Default for ControlInitiatorBook {
    fn default() -> Self {
        Self {
            tickets: AsyncTickets::new(CAPACITY),
            requests: HashMap::new(),
            ticket_requests: HashMap::new(),
            now_value: 0,
        }
    }
}

impl ControlInitiatorBook {
    pub fn allocate(
        &mut self,
        controller_id: ControllerId,
        kind: ControlRequestKind,
        now: u64,
        deadline: ControlDeadline,
    ) -> Result<ControlRequest, ControlInitiatorError> {
        if now < self.now_value {
            return Err(ControlInitiatorError::ClockRegressed);
        }
        self.now_value = now;
        let raw = NEXT_REQUEST
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| ControlInitiatorError::IdExhausted)?;
        let ticket = self
            .tickets
            .submit(now, deadline.0)
            .map_err(|error| match error {
                SubmitError::Full => ControlInitiatorError::Full,
                SubmitError::DeadlineElapsed => ControlInitiatorError::DeadlineElapsed,
                SubmitError::ClockRegressed => ControlInitiatorError::ClockRegressed,
                SubmitError::IdExhausted => ControlInitiatorError::IdExhausted,
            })?;
        let request = ControlRequest {
            id: ControlRequestId(u128::from(raw)),
            controller_id,
            kind,
            deadline,
        };
        self.requests.insert(
            request.id,
            RequestEntry {
                request,
                ticket,
                dispatch: DispatchState::Awaiting,
                preparing_seen: false,
                ready_sent: false,
                ticket_pending: true,
            },
        );
        self.ticket_requests.insert(ticket, request.id);
        Ok(request)
    }

    pub fn advance(&mut self, now: u64) -> Result<(), ControlInitiatorError> {
        self.tickets
            .advance(now)
            .map_err(|_| ControlInitiatorError::ClockRegressed)?;
        self.now_value = now;
        Ok(())
    }

    /// Accept at most one invitation for a live Acquire. The caller must clear
    /// old producer and prediction state before it sends Ready on stream zero.
    pub fn preparing(&mut self, invitation: ControlPreparing) -> bool {
        let now = self.now();
        let Some(entry) = self.requests.get_mut(&invitation.request_id) else {
            return false;
        };
        if entry.request.kind != ControlRequestKind::Acquire
            || entry.request.controller_id != invitation.controller_id
            || now >= entry.request.deadline.0
            || entry.preparing_seen
            || !entry.ticket_pending
        {
            return false;
        }
        entry.preparing_seen = true;
        true
    }

    pub fn invitation_live(&self, invitation: ControlPreparing) -> bool {
        self.requests
            .get(&invitation.request_id)
            .is_some_and(|entry| {
                entry.request.kind == ControlRequestKind::Acquire
                    && entry.request.controller_id == invitation.controller_id
                    && entry.preparing_seen
                    && entry.ticket_pending
                    && self.now() < entry.request.deadline.0
            })
    }

    pub fn can_terminal(&self, terminal: ControlTerminal) -> bool {
        self.requests
            .get(&terminal.request_id)
            .is_some_and(|entry| {
                entry.ticket_pending
                    && matches!(entry.dispatch, DispatchState::Reported)
                    && entry.request.controller_id == terminal.controller_id
                    && self.now() < entry.request.deadline.0
                    && match terminal.outcome {
                        Ok(ControlSuccess::Released) => {
                            entry.request.kind == ControlRequestKind::Release
                        }
                        Ok(ControlSuccess::Acquired(binding)) => {
                            binding.0 != 0
                                && entry.request.kind == ControlRequestKind::Acquire
                                && entry.ready_sent
                        }
                        Err(_) => true,
                    }
            })
    }

    pub fn can_ready(&self, invitation: ControlPreparing) -> bool {
        self.invitation_live(invitation)
            && self
                .requests
                .get(&invitation.request_id)
                .is_some_and(|entry| {
                    matches!(entry.dispatch, DispatchState::Reported) && !entry.ready_sent
                })
    }

    pub fn ready_sent(&mut self, invitation: ControlPreparing) {
        if self.can_ready(invitation) {
            self.requests
                .get_mut(&invitation.request_id)
                .expect("checked")
                .ready_sent = true;
        }
    }

    pub fn fail(&mut self, id: ControlRequestId) {
        if let Some(entry) = self.requests.get_mut(&id) {
            if entry.ticket_pending {
                entry.ticket_pending = false;
                let _ = self.tickets.reply(entry.ticket, self.now_value, Err(()));
            }
        }
    }

    pub fn terminal(&mut self, terminal: ControlTerminal) -> bool {
        if !self.can_terminal(terminal) {
            return false;
        }
        let now = self.now();
        let entry = self
            .requests
            .get_mut(&terminal.request_id)
            .expect("checked");
        let delivered = self
            .tickets
            .reply(entry.ticket, now, Ok(terminal.outcome))
            .is_ok();
        if delivered {
            entry.ticket_pending = false;
        }
        delivered
    }

    pub(crate) fn clock_floor(&self) -> u64 {
        self.now_value
    }

    // Stored time is advanced only by caller; no wall clock or server tick comparison.
    fn now(&self) -> u64 {
        self.now_value
    }

    pub fn dispatch(
        &mut self,
        id: ControlRequestId,
        now: u64,
        result: Result<(), ()>,
    ) -> Result<(), ControlInitiatorError> {
        self.advance(now)?;
        let entry = self
            .requests
            .get_mut(&id)
            .ok_or(ControlInitiatorError::UnknownRequest)?;
        let DispatchState::Awaiting = entry.dispatch else {
            return Err(ControlInitiatorError::UnknownRequest);
        };
        // Expiry wins over either report. A successful send alone does not
        // create a terminal result; an unreported terminal is never buffered.
        if result.is_err() {
            entry.ticket_pending = false;
            match self.tickets.reply(entry.ticket, now, Err(())) {
                Ok(()) | Err(ReplyError::NotPending) => {} // local timeout already queued
                Err(ReplyError::ClockRegressed) => {
                    return Err(ControlInitiatorError::ClockRegressed);
                }
            }
        }
        entry.dispatch = DispatchState::Reported;
        Ok(())
    }

    pub fn take(&mut self) -> Option<(ControlRequest, ControlInitiatorCompletion)> {
        let (ticket, result) = self.tickets.try_next()?;
        let id = self
            .ticket_requests
            .remove(&ticket)
            .expect("issued ticket has request");
        let request = self
            .requests
            .remove(&id)
            .expect("issued request remains")
            .request;
        let completion = match result {
            Completion::Success(outcome) => ControlInitiatorCompletion::Terminal(outcome),
            Completion::Error(()) | Completion::Cancelled => {
                ControlInitiatorCompletion::DispatchFailed
            }
            Completion::TimedOut => ControlInitiatorCompletion::TimedOut,
        };
        Some((request, completion))
    }
}
