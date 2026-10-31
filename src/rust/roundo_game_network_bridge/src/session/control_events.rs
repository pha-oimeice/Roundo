//! Controller FIFO receipts and sending share the lifecycle's private protocol state.

use super::*;
use roundo_contracts::control_handoff::{ControlPreparing, ControlRequestId, Ready};

impl ClientGameSessionLifecycle {
    /// Consumes one dequeued event. All receipt settlement and sends retain the
    /// lifecycle lock; the sender must not reenter this lifecycle. Its captured
    /// environment is dropped only after the guard, including during unwind.
    pub(crate) fn forward_controller_event(
        &self,
        event: ClientMarionetteEvent,
        now: u64,
        mut send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
        production_clock: bool,
    ) -> BridgeStep {
        match event {
            ClientMarionetteEvent::AcquireController {
                epoch,
                controller_id,
            } => {
                let result = if production_clock {
                    self.send_automatic_acquire_for_epoch_at_production_clock(
                        epoch,
                        controller_id,
                        &mut send,
                    )
                } else {
                    self.send_automatic_acquire_for_epoch(
                        epoch,
                        controller_id,
                        now,
                        ControlDeadline(now.saturating_add(5_000)),
                        &mut send,
                    )
                };
                if let Err(error) = result {
                    if error != ControlInitiatorError::StaleSession {
                        warn!(
                            "Automatic Acquire was not admitted: controller={}, error={error:?}",
                            controller_id.0
                        );
                    }
                }
                BridgeStep::Forwarded
            }
            ClientMarionetteEvent::ReleaseController { controller_id } => {
                warn!(
                    "Dropped legacy Marionette Release request: controller={}",
                    controller_id.0
                );
                BridgeStep::Forwarded
            }
            event => self.forward_session_event(event, &mut send),
        }
    }

    fn forward_session_event(
        &self,
        event: ClientMarionetteEvent,
        send: &mut impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> BridgeStep {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(epoch) = state.active_epoch else {
            return BridgeStep::Idle;
        };
        if let ClientMarionetteEvent::SessionActivated { epoch: observed } = event {
            if epoch == observed {
                state.activated_epoch = Some(epoch);
            }
            return BridgeStep::Forwarded;
        }
        if state.activated_epoch != Some(epoch) {
            // The new producer must cross its ordered activation fence first.
            return BridgeStep::Forwarded;
        }
        match event {
            ClientMarionetteEvent::BindingClosureConfirmed {
                epoch,
                request_id,
                controller_id,
            } => {
                Self::confirm_binding_closure(&mut state, epoch, request_id, controller_id);
            }
            ClientMarionetteEvent::BindingActivated {
                epoch,
                request_id,
                controller_id,
            } => {
                self.confirm_binding_activation(&mut state, epoch, request_id, controller_id);
            }
            ClientMarionetteEvent::PreparingCleaned { epoch, preparing } => {
                return Self::send_control_ready(&mut state, epoch, preparing, send);
            }
            ClientMarionetteEvent::DrainBarrier(ref barrier) if epoch != barrier.session_epoch => {
                warn!("Dropped stale client drain barrier at session boundary");
            }
            event => {
                let blocked = match &event {
                    ClientMarionetteEvent::SubmitControllerInput { controller_id, .. } => {
                        state.blocked_controllers.contains(controller_id)
                    }
                    ClientMarionetteEvent::UsePlayerController(_) => {
                        !state.blocked_controllers.is_empty()
                    }
                    _ => false,
                };
                // Never use a legacy control route as a ticket fallback.
                if !blocked && send(controller_event_message(event)).is_err() {
                    return BridgeStep::Stop;
                }
            }
        }
        BridgeStep::Forwarded
    }

    fn confirm_binding_closure(
        state: &mut ClientGameSessionState,
        epoch: u64,
        request_id: ControlRequestId,
        controller: ControllerId,
    ) {
        if let Some((pending_epoch, terminal)) = state.closure_pending.get(&controller).copied()
            && state.active_epoch == Some(epoch)
            && pending_epoch == epoch
            && terminal.request_id == request_id
            && state.control_book.terminal(terminal)
        {
            state.closure_pending.remove(&controller);
        }
    }

    fn confirm_binding_activation(
        &self,
        state: &mut ClientGameSessionState,
        epoch: u64,
        request_id: ControlRequestId,
        controller: ControllerId,
    ) {
        let Some((pending_epoch, terminal)) = state.activation_pending.get(&controller).copied()
        else {
            return;
        };
        if state.active_epoch != Some(epoch)
            || pending_epoch != epoch
            || terminal.request_id != request_id
            || !state.control_book.can_terminal(terminal)
        {
            return;
        }
        if self
            .marionette
            .try_send(ClientMarionetteCommand::TicketActivated {
                epoch,
                request_id,
                controller_id: controller,
            })
            .is_err()
        {
            state.control_book.fail(request_id);
            state.activation_pending.remove(&controller);
            return;
        }
        state.control_book.terminal(terminal);
        state.activation_pending.remove(&controller);
        state.preparing.retain(|item| item.request_id != request_id);
        state.blocked_controllers.remove(&controller);
    }

    fn send_control_ready(
        state: &mut ClientGameSessionState,
        epoch: u64,
        preparing: ControlPreparing,
        send: &mut impl FnMut(ClientGameMessage) -> Result<(), ()>,
    ) -> BridgeStep {
        if state.active_epoch != Some(epoch) || !state.control_book.can_ready(preparing) {
            return BridgeStep::Forwarded;
        }
        if send(ClientGameMessage::Ready(Ready {
            request_id: preparing.request_id,
            controller_id: preparing.controller_id,
        }))
        .is_err()
        {
            state.control_book.fail(preparing.request_id);
            state
                .preparing
                .retain(|item| item.request_id != preparing.request_id);
            return BridgeStep::Stop;
        }
        state.control_book.ready_sent(preparing);
        BridgeStep::Forwarded
    }
}
