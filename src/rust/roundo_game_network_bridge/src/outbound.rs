//! Ordered client ECS polling and stream-0 sending; the lifecycle owns control protocol state.

use super::*;

/// Forwards sequenced controller events to stream 0 until shutdown or send failure.
///
/// The event that encounters saturation/closure is dropped, and the bridge stops
/// permanently; it does not retry after network reconnection.
pub fn bridge_game_ecs_events(
    marionette_ipc: ClientMarionetteIpc,
    session_lifecycle: ClientGameSessionLifecycle,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started client game IPC bridge");
    run_polling_bridge(&stop, || {
        bridge_game_event_step(&marionette_ipc, &session_lifecycle, |message| {
            network.send(message).map_err(|error| {
                warn!("Stopped client network bridge after send failure: error={error}");
            })
        })
    });
    debug!("Stopped client game IPC bridge");
}

/// One production bridge step using the supplied stream-0 sender and lifecycle.
pub fn bridge_game_event_step(
    marionette_ipc: &ClientMarionetteIpc,
    session_lifecycle: &ClientGameSessionLifecycle,
    send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
) -> BridgeStep {
    let now = match session_lifecycle.advance_production_control_clock() {
        Ok(now) => now,
        Err(error) => {
            warn!("Client control clock stopped: error={error:?}");
            return BridgeStep::Stop;
        }
    };
    bridge_game_event_step_after_advance_at_time(marionette_ipc, session_lifecycle, now, send, true)
}

/// One complete production bridge step using caller-supplied lifecycle-local
/// milliseconds. Callers must use one nondecreasing local clock, never server ticks.
pub fn bridge_game_event_step_at_time(
    marionette_ipc: &ClientMarionetteIpc,
    session_lifecycle: &ClientGameSessionLifecycle,
    now: u64,
    send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
) -> BridgeStep {
    if let Err(error) = session_lifecycle.advance_control_time(now) {
        warn!("Client control clock stopped: error={error:?}");
        return BridgeStep::Stop;
    }
    bridge_game_event_step_after_advance_at_time(
        marionette_ipc,
        session_lifecycle,
        now,
        send,
        false,
    )
}

fn bridge_game_event_step_after_advance_at_time(
    marionette_ipc: &ClientMarionetteIpc,
    session_lifecycle: &ClientGameSessionLifecycle,
    now: u64,
    send: impl FnMut(ClientGameMessage) -> Result<(), ()>,
    production_clock: bool,
) -> BridgeStep {
    while session_lifecycle.deliver_next_control_completion() {}
    let Some(event) = marionette_ipc.try_receive() else {
        return BridgeStep::Idle;
    };
    session_lifecycle.forward_controller_event(event, now, send, production_clock)
}

pub fn controller_event_message(event: ClientMarionetteEvent) -> ClientGameMessage {
    match event {
        ClientMarionetteEvent::RequestPlayerControllerAccess => {
            ClientGameMessage::RequestPlayerControllerAccess
        }
        ClientMarionetteEvent::AcquireController { .. } => {
            unreachable!("automatic Acquire is handled only by the production ticketed bridge")
        }
        ClientMarionetteEvent::ReleaseController { controller_id } => {
            // Legacy unbound release mapping; not the ticketed transition path.
            ClientGameMessage::ReleaseController { controller_id }
        }
        ClientMarionetteEvent::SubmitControllerInput {
            controller_id,
            input,
        } => ClientGameMessage::SubmitControllerInput {
            controller_id,
            input,
        },
        ClientMarionetteEvent::DrainBarrier(barrier) => ClientGameMessage::DrainBarrier(barrier),
        ClientMarionetteEvent::SessionActivated { .. }
        | ClientMarionetteEvent::PreparingCleaned { .. }
        | ClientMarionetteEvent::BindingActivated { .. }
        | ClientMarionetteEvent::BindingClosureConfirmed { .. } => {
            unreachable!("receipt handled by lifecycle owner")
        }
        ClientMarionetteEvent::UsePlayerController(command) => {
            ClientGameMessage::UsePlayerController { command }
        }
    }
}
