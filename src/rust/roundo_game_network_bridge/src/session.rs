//! Linearized client authority-session lifecycle and ticket projections.

use super::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Mutex,
    time::Instant,
};

mod control_events;
mod debug;
mod inventory;
pub use debug::{DebugRequestStatus, DebugStatusSnapshot};
pub use inventory::InventoryView;

/// Owns the complete client-side authority-session seam.
///
/// Clones share one lifecycle mutex. Inbound messages, control receipts and
/// stream-0 sends cannot cross an end/begin transition while it is held; each
/// domain observes FIFO lifecycle commands around its session-scoped payloads.
#[derive(Clone)]
pub struct ClientGameSessionLifecycle {
    state: Arc<Mutex<ClientGameSessionState>>,
    marionette: ClientMarionetteIpc,
    presence: ClientPresenceIpc,
    local_coordinate: LocalCoordinateClientIpc,
    control_clock: Instant,
}

#[derive(Default)]
struct ClientGameSessionState {
    next_epoch: u64,
    active_epoch: Option<u64>,
    activated_epoch: Option<u64>,
    control_book: ControlInitiatorBook,
    preparing: VecDeque<roundo_contracts::control_handoff::ControlPreparing>,
    blocked_controllers: HashSet<ControllerId>,
    /// One automatic request per selected controller and session; only an
    /// explicit retry or a new session may start another after failure.
    automatic_acquires: HashSet<ControllerId>,
    activation_pending:
        HashMap<ControllerId, (u64, roundo_contracts::control_handoff::ControlTerminal)>,
    closure_pending:
        HashMap<ControllerId, (u64, roundo_contracts::control_handoff::ControlTerminal)>,
    acquire_states: HashMap<ControllerId, AcquireRecord>,
    debug: debug::DebugRequests,
    inventory: inventory::InventoryObservation,
}

const RETRY_CONTROLLER_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
struct AcquireRecord {
    epoch: u64,
    request_id: ControlRequestId,
    state: ControlAcquireState,
}

impl ClientGameSessionLifecycle {
    pub fn new(
        marionette: ClientMarionetteIpc,
        presence: ClientPresenceIpc,
        local_coordinate: LocalCoordinateClientIpc,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(ClientGameSessionState::default())),
            marionette,
            presence,
            local_coordinate,
            control_clock: Instant::now(),
        }
    }

    /// Returns this lifecycle owner's monotonic local time in milliseconds.
    /// It survives session replacement so a newly established session cannot
    /// move the ticket book's clock backward.
    pub fn control_time_millis(&self) -> u64 {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        Self::control_time_millis_locked(&self.control_clock, &state)
    }

    fn control_time_millis_locked(clock: &Instant, state: &ClientGameSessionState) -> u64 {
        let elapsed = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        elapsed.max(state.control_book.clock_floor())
    }

    /// Advances the production clock while the lifecycle mutex owns both its
    /// elapsed-time sample and the ticket-book floor. Deterministic callers
    /// must continue to use their explicit `_at_time` seam instead.
    pub(super) fn advance_production_control_clock(&self) -> Result<u64, ControlInitiatorError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let now = Self::control_time_millis_locked(&self.control_clock, &state);
        state.control_book.advance(now)?;
        Ok(now)
    }

    pub fn begin(&self) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(epoch) = state.active_epoch.take() {
            self.publish_end(epoch);
        }
        state.activated_epoch = None;
        state.control_book = ControlInitiatorBook::default();
        state.preparing.clear();
        state.blocked_controllers.clear();
        state.automatic_acquires.clear();
        state.activation_pending.clear();
        state.closure_pending.clear();
        state.acquire_states.clear();
        let mut discarded_controller_events = 0;
        while self.marionette.try_receive().is_some() {
            discarded_controller_events += 1;
        }
        let mut discarded_resource_events = 0;
        while self.local_coordinate.try_receive().is_some() {
            discarded_resource_events += 1;
        }
        if discarded_controller_events != 0 || discarded_resource_events != 0 {
            warn!(
                "Discarded stale ECS events at client session boundary: controller={discarded_controller_events}, resource={discarded_resource_events}"
            );
        }
        state.next_epoch = state.next_epoch.wrapping_add(1).max(1);
        let epoch = state.next_epoch;
        state.debug.clear();
        state.inventory.clear();
        self.publish_begin(epoch);
        state.active_epoch = Some(epoch);
        epoch
    }

    /// Observes the current session without draining domain command queues.
    pub fn active_epoch(&self) -> Option<u64> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active_epoch
    }

    pub fn end(&self) -> Option<u64> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let epoch = state.active_epoch.take()?;
        state.activated_epoch = None;
        state.control_book = ControlInitiatorBook::default();
        state.preparing.clear();
        state.blocked_controllers.clear();
        state.automatic_acquires.clear();
        state.activation_pending.clear();
        state.closure_pending.clear();
        state.acquire_states.clear();
        state.debug.clear();
        state.inventory.clear();
        self.publish_end(epoch);
        Some(epoch)
    }

    pub fn route_game(&self, message: ServerGameMessage) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.active_epoch.is_none() {
            warn!("Dropped server game message outside an active client session");
            return;
        }
        let epoch = state.active_epoch.expect("checked above");
        match message {
            ServerGameMessage::Inventory(reply) => state.inventory.receive(reply),
            ServerGameMessage::Debug(reply) => state.debug.receive(reply),
            ServerGameMessage::ControlPreparing(preparing) => {
                if state.control_book.preparing(preparing) {
                    // Block the outbound bridge before publishing the ECS command:
                    // old queued Submit events may still be waiting in its FIFO.
                    state.preparing.push_back(preparing);
                    state.blocked_controllers.insert(preparing.controller_id);
                    state.activation_pending.remove(&preparing.controller_id);
                    if self
                        .marionette
                        .try_send(ClientMarionetteCommand::ControlPreparing { epoch, preparing })
                        .is_err()
                    {
                        state.control_book.fail(preparing.request_id);
                        state
                            .preparing
                            .retain(|item| item.request_id != preparing.request_id);
                        warn!(
                            "Failed to forward ticketed Preparing; ticket closed, producer blocked at bridge"
                        );
                    } else if let Some(record) =
                        state.acquire_states.get_mut(&preparing.controller_id)
                        && record.epoch == epoch
                        && record.request_id == preparing.request_id
                    {
                        record.state = ControlAcquireState::Preparing;
                    }
                } else {
                    warn!(
                        "Dropped unclaimed Acquire invitation: request_id={}",
                        preparing.request_id.0
                    );
                }
            }
            ServerGameMessage::ControlTerminal(terminal) => {
                if state.closure_pending.contains_key(&terminal.controller_id)
                    || (state
                        .activation_pending
                        .contains_key(&terminal.controller_id)
                        && matches!(
                            terminal.outcome,
                            Ok(roundo_contracts::control_handoff::ControlSuccess::Acquired(
                                _
                            ))
                        ))
                    || !state.control_book.can_terminal(terminal)
                {
                    warn!(
                        "Dropped unclaimed ticketed terminal: request_id={}",
                        terminal.request_id.0
                    );
                } else {
                    let command = match terminal.outcome {
                        Ok(roundo_contracts::control_handoff::ControlSuccess::Acquired(
                            binding,
                        )) => ClientMarionetteCommand::TicketAcquired {
                            epoch,
                            request_id: terminal.request_id,
                            controller_id: terminal.controller_id,
                            binding,
                        },
                        _ => ClientMarionetteCommand::TicketReleased {
                            epoch,
                            request_id: terminal.request_id,
                            controller_id: terminal.controller_id,
                        },
                    };
                    if !matches!(
                        terminal.outcome,
                        Ok(roundo_contracts::control_handoff::ControlSuccess::Acquired(
                            _
                        ))
                    ) {
                        state.blocked_controllers.insert(terminal.controller_id);
                        if let Some((_, superseded)) =
                            state.activation_pending.remove(&terminal.controller_id)
                        {
                            state.control_book.fail(superseded.request_id);
                        }
                    }
                    if self.marionette.try_send(command).is_err() {
                        state.control_book.fail(terminal.request_id);
                        state.blocked_controllers.insert(terminal.controller_id);
                        state.activation_pending.remove(&terminal.controller_id);
                    } else if matches!(
                        terminal.outcome,
                        Ok(roundo_contracts::control_handoff::ControlSuccess::Acquired(
                            _
                        ))
                    ) {
                        state
                            .activation_pending
                            .insert(terminal.controller_id, (epoch, terminal));
                    } else if matches!(
                        terminal.outcome,
                        Ok(roundo_contracts::control_handoff::ControlSuccess::Released)
                    ) {
                        state
                            .closure_pending
                            .insert(terminal.controller_id, (epoch, terminal));
                    } else if state.control_book.terminal(terminal) {
                        state
                            .preparing
                            .retain(|item| item.request_id != terminal.request_id);
                    }
                }
            }
            ServerGameMessage::Freeze(freeze) => {
                if self
                    .marionette
                    .try_send(ClientMarionetteCommand::Freeze { epoch, freeze })
                    .is_err()
                {
                    warn!(
                        "Failed to forward Freeze to Marionette: epoch={epoch}, request_id={}",
                        freeze.request_id.0
                    );
                }
            }
            ServerGameMessage::PlayerControllerAccessSnapshot { snapshot } => {
                let _ = self.marionette.try_send(
                    ClientMarionetteCommand::PlayerControllerAccessSnapshot { epoch, snapshot },
                );
            }
            ServerGameMessage::ControllerOperationRejected { operation, error } => {
                let _ = self.marionette.try_send(
                    ClientMarionetteCommand::ControllerOperationRejected {
                        epoch,
                        operation,
                        error,
                    },
                );
            }
            ServerGameMessage::ControllerAcquired { controller_id } => {
                let _ = self
                    .marionette
                    .try_send(ClientMarionetteCommand::ControllerAcquired {
                        epoch,
                        controller_id,
                    });
            }
            ServerGameMessage::ControllerReleased { controller_id } => {
                let _ = self
                    .marionette
                    .try_send(ClientMarionetteCommand::ControllerReleased {
                        epoch,
                        controller_id,
                    });
            }
            ServerGameMessage::CreatureMotionSnapshot { snapshot } => {
                if self
                    .marionette
                    .try_send(ClientMarionetteCommand::CreatureMotionSnapshot(snapshot))
                    .is_err()
                {
                    warn!("Dropped CreatureMotionSnapshot message: destination=marionette_ecs");
                }
            }
            ServerGameMessage::PlayerState { state } => {
                if self
                    .marionette
                    .try_send(ClientMarionetteCommand::PlayerState(state))
                    .is_err()
                {
                    warn!("Dropped PlayerState message: destination=marionette_ecs");
                }
            }
            ServerGameMessage::PresenceSnapshot { snapshot } => {
                if self
                    .presence
                    .try_send(ClientPresenceCommand::Snapshot(snapshot))
                    .is_err()
                {
                    warn!("Dropped PresenceSnapshot message: destination=presence_ecs");
                }
            }
        }
    }

    /// Returns a correlated Acquire invitation, not permission to send Ready yet.
    /// The caller must discard old pending input and prediction before sending Ready.
    pub fn take_control_preparing(
        &self,
    ) -> Option<roundo_contracts::control_handoff::ControlPreparing> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while let Some(invitation) = state.preparing.pop_front() {
            if state.control_book.invitation_live(invitation) {
                return Some(invitation);
            }
        }
        None
    }

    /// Atomically validates the FIFO epoch, deduplicates the selected
    /// controller, and dispatches its ticketed Acquire on stream 0.
    ///
    /// A failed dispatch remains reserved for this epoch and completes through
    /// the normal result path; repeated snapshots never create a retry storm.
    pub fn send_automatic_acquire_for_epoch(
        &self,
        event_epoch: u64,
        controller: ControllerId,
        now: u64,
        deadline: ControlDeadline,
        mut send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<Option<ControlRequest>, ControlInitiatorError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        Self::send_automatic_acquire_locked(
            &mut state,
            event_epoch,
            controller,
            now,
            deadline,
            &mut send,
        )
    }

    /// Production automatic Acquire samples elapsed time, reads the ticket
    /// floor, and allocates while one lifecycle lock is held.
    pub(super) fn send_automatic_acquire_for_epoch_at_production_clock(
        &self,
        event_epoch: u64,
        controller: ControllerId,
        mut send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<Option<ControlRequest>, ControlInitiatorError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let now = Self::control_time_millis_locked(&self.control_clock, &state);
        let deadline = ControlDeadline(now.saturating_add(5_000));
        Self::send_automatic_acquire_locked(
            &mut state,
            event_epoch,
            controller,
            now,
            deadline,
            &mut send,
        )
    }

    fn send_automatic_acquire_locked(
        state: &mut ClientGameSessionState,
        event_epoch: u64,
        controller: ControllerId,
        now: u64,
        deadline: ControlDeadline,
        send: &mut impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<Option<ControlRequest>, ControlInitiatorError> {
        if state.active_epoch != Some(event_epoch) || state.activated_epoch != Some(event_epoch) {
            return Err(ControlInitiatorError::StaleSession);
        }
        if !state.automatic_acquires.insert(controller) {
            return Ok(None);
        }
        if let Err(error) =
            Self::reserve_acquire_state(state, event_epoch, controller, ControlRequestId(0))
        {
            state.automatic_acquires.remove(&controller);
            return Err(error);
        }
        let request = match state.control_book.allocate(
            controller,
            ControlRequestKind::Acquire,
            now,
            deadline,
        ) {
            Ok(request) => request,
            Err(error) => {
                state.acquire_states.remove(&controller);
                return Err(error);
            }
        };
        state
            .acquire_states
            .get_mut(&controller)
            .expect("reserved")
            .request_id = request.id;
        let result = send(ClientGameMessage::ControlRequest(request));
        state.control_book.dispatch(request.id, now, result)?;
        state.blocked_controllers.insert(controller);
        Ok(Some(request))
    }

    /// Returns the active epoch's lifecycle-owned control projection. No
    /// transport, binding, or ECS-internal identity is exposed.
    pub fn control_acquire_status(&self, controller: ControllerId) -> Option<ControlAcquireStatus> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let epoch = state.active_epoch?;
        let record = state.acquire_states.get(&controller).copied();
        Some(ControlAcquireStatus {
            epoch,
            controller_id: controller,
            request_id: record.map(|entry| entry.request_id).filter(|id| id.0 != 0),
            state: record.map_or(ControlAcquireState::Idle, |entry| entry.state),
        })
    }

    /// Returns every tracked Acquire in the active epoch, ordered by controller.
    /// This read-only UI projection deliberately omits Idle controllers and all
    /// binding/ECS identities.
    pub fn control_acquire_statuses(&self) -> Vec<ControlAcquireStatus> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(epoch) = state.active_epoch else {
            return Vec::new();
        };
        let mut statuses: Vec<_> = state
            .acquire_states
            .iter()
            .filter_map(|(&controller_id, &record)| {
                (record.epoch == epoch).then_some(ControlAcquireStatus {
                    epoch,
                    controller_id,
                    request_id: (record.request_id.0 != 0).then_some(record.request_id),
                    state: record.state,
                })
            })
            .collect();
        statuses.sort_unstable_by_key(|status| status.controller_id);
        statuses
    }

    /// Explicitly starts one new ticketed Acquire only after this lifecycle has
    /// delivered a failed Acquire completion in the active, activated epoch.
    /// The supplied stream-0 sender uses this lifecycle's admission and correlation.
    pub fn retry_failed_acquire_with_sender(
        &self,
        controller: ControllerId,
        send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<ControlRequest, ControlRetryError> {
        self.retry_failed_acquire_with_time_source_and_sender(
            controller,
            || u64::try_from(self.control_clock.elapsed().as_millis()).unwrap_or(u64::MAX),
            send,
        )
    }

    fn retry_failed_acquire_with_time_source_and_sender(
        &self,
        controller: ControllerId,
        elapsed: impl FnOnce() -> u64,
        mut send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<ControlRequest, ControlRetryError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        // Sample elapsed time and read the book floor under the same lifecycle
        // lock that allocates the retry. This is deliberately not a relaxation
        // of explicit-time APIs: those retain ClockRegressed validation.
        let now = elapsed().max(state.control_book.clock_floor());
        let deadline = ControlDeadline(now.saturating_add(5_000));
        let Some(epoch) = state.active_epoch else {
            return Err(ControlRetryError::StaleSession);
        };
        if state.activated_epoch != Some(epoch) {
            return Err(ControlRetryError::StaleSession);
        }
        match state.acquire_states.get(&controller).copied() {
            Some(AcquireRecord {
                epoch: record_epoch,
                state: ControlAcquireState::Failed,
                ..
            }) if record_epoch == epoch => {}
            Some(AcquireRecord {
                state: ControlAcquireState::Pending | ControlAcquireState::Preparing,
                ..
            }) => return Err(ControlRetryError::InProgress),
            Some(AcquireRecord {
                state: ControlAcquireState::Active,
                ..
            }) => return Err(ControlRetryError::Active),
            _ => return Err(ControlRetryError::NotFailed),
        }
        let request = state
            .control_book
            .allocate(controller, ControlRequestKind::Acquire, now, deadline)
            .map_err(ControlRetryError::Initiator)?;
        state.acquire_states.insert(
            controller,
            AcquireRecord {
                epoch,
                request_id: request.id,
                state: ControlAcquireState::Pending,
            },
        );
        let result = send(ClientGameMessage::ControlRequest(request));
        state
            .control_book
            .dispatch(request.id, now, result)
            .map_err(ControlRetryError::Initiator)?;
        state.blocked_controllers.insert(controller);
        Ok(request)
    }

    /// Test-only deterministic clock seam. `elapsed_before_book_advance`
    /// represents a clock value observed before another production step moved
    /// the book floor; retry must still allocate from the lock-owned floor.
    #[cfg(feature = "client-test")]
    pub fn retry_failed_acquire_after_observed_time_with_sender(
        &self,
        controller: ControllerId,
        elapsed_before_book_advance: u64,
        send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<ControlRequest, ControlRetryError> {
        self.retry_failed_acquire_with_time_source_and_sender(
            controller,
            || elapsed_before_book_advance,
            send,
        )
    }

    /// Uses the same production stream-0 send as all ticketed control traffic.
    pub fn retry_failed_acquire(
        &self,
        network: &ClientNetwork,
        controller: ControllerId,
    ) -> Result<ControlRequest, ControlRetryError> {
        self.retry_failed_acquire_with_sender(controller, |message| {
            network.send(message).map_err(|_| ())
        })
    }

    fn reserve_acquire_state(
        state: &mut ClientGameSessionState,
        epoch: u64,
        controller: ControllerId,
        request_id: ControlRequestId,
    ) -> Result<(), ControlInitiatorError> {
        if let Some(record) = state.acquire_states.get(&controller) {
            return Err(match record.state {
                ControlAcquireState::Pending | ControlAcquireState::Preparing => {
                    ControlInitiatorError::InProgress
                }
                ControlAcquireState::Active => ControlInitiatorError::Active,
                // A failed request remains the retry correlation until explicit
                // retry replaces it; a fresh Acquire must not overwrite it.
                ControlAcquireState::Failed => ControlInitiatorError::RetryRequired,
                ControlAcquireState::Idle => unreachable!("Idle is not stored"),
            });
        }
        if state.acquire_states.len() == RETRY_CONTROLLER_CAPACITY {
            return Err(ControlInitiatorError::Full);
        }
        state.acquire_states.insert(
            controller,
            AcquireRecord {
                epoch,
                request_id,
                state: ControlAcquireState::Pending,
            },
        );
        Ok(())
    }

    /// Atomically allocates and dispatches on the actual production stream-0
    /// transport under the session lock. A failed enqueue completes locally;
    /// it never falls back to the unsafe legacy Acquire/Release route.
    pub fn send_control_request(
        &self,
        network: &ClientNetwork,
        controller: ControllerId,
        kind: ControlRequestKind,
        now: u64,
        deadline: ControlDeadline,
    ) -> Result<ControlRequest, ControlInitiatorError> {
        self.send_control_request_with_sender(controller, kind, now, deadline, |message| {
            network.send(message).map_err(|_| ())
        })
    }

    /// Uses an injected stream-0 sender with the same Acquire admission and
    /// completion projection as `send_control_request`.
    pub fn send_control_request_with_sender(
        &self,
        controller: ControllerId,
        kind: ControlRequestKind,
        now: u64,
        deadline: ControlDeadline,
        mut send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> Result<ControlRequest, ControlInitiatorError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(epoch) = state.active_epoch else {
            return Err(ControlInitiatorError::StaleSession);
        };
        if state.activated_epoch != Some(epoch) {
            return Err(ControlInitiatorError::StaleSession);
        }
        if kind == ControlRequestKind::Acquire {
            Self::reserve_acquire_state(&mut state, epoch, controller, ControlRequestId(0))?;
        }
        let request = match state.control_book.allocate(controller, kind, now, deadline) {
            Ok(request) => request,
            Err(error) => {
                if kind == ControlRequestKind::Acquire {
                    state.acquire_states.remove(&controller);
                }
                return Err(error);
            }
        };
        if kind == ControlRequestKind::Acquire {
            state
                .acquire_states
                .get_mut(&controller)
                .expect("reserved Acquire state")
                .request_id = request.id;
        }
        let result = send(ClientGameMessage::ControlRequest(request));
        if let Err(error) = state.control_book.dispatch(request.id, now, result) {
            if kind == ControlRequestKind::Acquire {
                state.acquire_states.remove(&controller);
            }
            return Err(error);
        }
        if kind == ControlRequestKind::Acquire {
            state.blocked_controllers.insert(controller);
        }
        Ok(request)
    }

    /// Allocates correlation only. The caller must dispatch `ClientGameMessage::ControlRequest`
    /// and report the send result. Deadline is local bookkeeping, not a server commit clock.
    pub fn allocate_control_request(
        &self,
        epoch: u64,
        controller: ControllerId,
        kind: ControlRequestKind,
        now: u64,
        deadline: ControlDeadline,
    ) -> Result<ControlRequest, ControlInitiatorError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.active_epoch != Some(epoch) {
            return Err(ControlInitiatorError::StaleSession);
        }
        // This is deliberately correlation-only: callers that use it retain
        // their existing dispatch/report contract and it does not become a
        // second Acquire admission path. Production Acquire uses the shared
        // send path above, which records the UI retry projection atomically.
        state.control_book.allocate(controller, kind, now, deadline)
    }

    pub fn report_control_dispatch(
        &self,
        epoch: u64,
        id: ControlRequestId,
        now: u64,
        result: Result<(), ()>,
    ) -> Result<(), ControlInitiatorError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.active_epoch != Some(epoch) {
            return Err(ControlInitiatorError::StaleSession);
        }
        let reported = state.control_book.dispatch(id, now, result);
        if reported.is_ok() && result.is_err() {
            state.preparing.retain(|item| item.request_id != id);
        }
        reported
    }

    pub fn advance_control_time(&self, now: u64) -> Result<(), ControlInitiatorError> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .control_book
            .advance(now)
    }

    /// Alternate direct completion consumer. It is mutually exclusive with the
    /// production bridge consumer: a completion is removed exactly once, and
    /// this path settles the same lifecycle projection before returning it.
    pub fn take_control_completion(&self) -> Option<(ControlRequest, ControlInitiatorCompletion)> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let (request, completion) = Self::take_control_completion_locked(&mut state)?;
        Self::settle_control_completion(&mut state, request, completion);
        Some((request, completion))
    }

    /// Reclaim only the consumed request's invitation, even if completion IPC
    /// fails or there is no active epoch. Both consumers hold the lifecycle lock.
    /// This does not change projection, retry eligibility, or activation blocks.
    fn take_control_completion_locked(
        state: &mut ClientGameSessionState,
    ) -> Option<(ControlRequest, ControlInitiatorCompletion)> {
        let (request, completion) = state.control_book.take()?;
        state.preparing.retain(|item| item.request_id != request.id);
        Some((request, completion))
    }

    fn settle_control_completion(
        state: &mut ClientGameSessionState,
        request: ControlRequest,
        completion: ControlInitiatorCompletion,
    ) {
        state
            .activation_pending
            .retain(|_, (_, terminal)| terminal.request_id != request.id);
        state
            .closure_pending
            .retain(|_, (_, terminal)| terminal.request_id != request.id);
        let Some(epoch) = state.active_epoch else {
            return;
        };
        let acquire_result = if request.kind == ControlRequestKind::Acquire {
            match completion {
                ControlInitiatorCompletion::Terminal(Ok(
                    roundo_contracts::control_handoff::ControlSuccess::Acquired(_),
                )) => Some(ControlAcquireState::Active),
                ControlInitiatorCompletion::Terminal(Err(_))
                | ControlInitiatorCompletion::DispatchFailed
                | ControlInitiatorCompletion::TimedOut => Some(ControlAcquireState::Failed),
                ControlInitiatorCompletion::Terminal(Ok(_)) => None,
            }
        } else {
            None
        };
        if let Some(next_state) = acquire_result
            && let Some(record) = state.acquire_states.get_mut(&request.controller_id)
            && record.epoch == epoch
            && record.request_id == request.id
        {
            record.state = next_state;
        } else if request.kind == ControlRequestKind::Release
            && matches!(
                completion,
                ControlInitiatorCompletion::Terminal(Ok(
                    roundo_contracts::control_handoff::ControlSuccess::Released
                ))
            )
        {
            state.acquire_states.remove(&request.controller_id);
            state.automatic_acquires.remove(&request.controller_id);
        }
    }

    /// Delivers one completion while retaining the lifecycle mutex from ticket
    /// removal through epoch capture and Marionette FIFO enqueue. Consequently
    /// an end/begin transition cannot project an old completion into a new
    /// session.
    pub(super) fn deliver_next_control_completion(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some((request, completion)) = Self::take_control_completion_locked(&mut state) else {
            return false;
        };
        let Some(epoch) = state.active_epoch else {
            Self::settle_control_completion(&mut state, request, completion);
            return true;
        };
        let outcome = match completion {
            ControlInitiatorCompletion::Terminal(outcome) => {
                ClientControlOutcome::Terminal(outcome)
            }
            ControlInitiatorCompletion::DispatchFailed => ClientControlOutcome::DispatchFailed,
            ControlInitiatorCompletion::TimedOut => ClientControlOutcome::TimedOut,
        };
        if self
            .marionette
            .try_send(ClientMarionetteCommand::TicketCompleted {
                epoch,
                request_id: request.id,
                controller_id: request.controller_id,
                outcome,
            })
            .is_err()
        {
            // Do not manufacture retry eligibility when the user-facing owner
            // cannot receive the failure result. Keeping Pending fails closed.
            warn!(
                "Dropped client control completion after Marionette shutdown: request_id={}",
                request.id.0
            );
        } else {
            // State becomes externally visible only after its IPC result is
            // enqueued, so failed Marionette delivery cannot grant retry or
            // release cleanup eligibility.
            Self::settle_control_completion(&mut state, request, completion);
        }
        true
    }

    /// Runs `f` only within an active session, holding the lifecycle mutex until
    /// it returns. The callback must not reenter this lifecycle or block waiting
    /// for work that requires it; no mutable protocol state escapes this seam.
    pub fn with_active<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active_epoch?;
        Some(f())
    }

    fn publish_begin(&self, epoch: u64) {
        if self
            .marionette
            .try_send(ClientMarionetteCommand::BeginSession { epoch })
            .is_err()
        {
            warn!("Failed to begin Marionette client session: epoch={epoch}");
        }
        if self
            .presence
            .try_send(ClientPresenceCommand::BeginSession { epoch })
            .is_err()
        {
            warn!("Failed to begin Presence client session: epoch={epoch}");
        }
        if self
            .local_coordinate
            .try_send(LocalCoordinateClientCommand::BeginSession { epoch })
            .is_err()
        {
            warn!("Failed to begin Local Coordinate client session: epoch={epoch}");
        }
    }

    fn publish_end(&self, epoch: u64) {
        if self
            .marionette
            .try_send(ClientMarionetteCommand::EndSession { epoch })
            .is_err()
        {
            warn!("Failed to end Marionette client session: epoch={epoch}");
        }
        if self
            .presence
            .try_send(ClientPresenceCommand::EndSession { epoch })
            .is_err()
        {
            warn!("Failed to end Presence client session: epoch={epoch}");
        }
        if self
            .local_coordinate
            .try_send(LocalCoordinateClientCommand::EndSession { epoch })
            .is_err()
        {
            warn!("Failed to end Local Coordinate client session: epoch={epoch}");
        }
    }
}
