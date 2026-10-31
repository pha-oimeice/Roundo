//! Production client/server wire adapters and session lifecycle boundaries.
#[cfg(feature = "client")]
mod control_initiator;
#[cfg(feature = "client")]
mod inbound;
#[cfg(feature = "client")]
mod outbound;
#[cfg(feature = "client")]
mod session;
#[cfg(feature = "client")]
pub use control_initiator::{ControlInitiatorCompletion, ControlInitiatorError};
#[cfg(feature = "client")]
pub use inbound::ClientHooksAdapter;
#[cfg(feature = "client")]
pub use outbound::{
    bridge_game_ecs_events, bridge_game_event_step, bridge_game_event_step_at_time,
    controller_event_message,
};
#[cfg(feature = "client")]
pub use session::{
    ClientGameSessionLifecycle, DebugRequestStatus, DebugStatusSnapshot, InventoryView,
};

/// Read-only lifecycle-owned state for one controller's ticketed Acquire.
/// This is a UI/manager projection only; it exposes no binding or ECS entity.
#[cfg(feature = "client")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlAcquireState {
    Idle,
    Pending,
    Preparing,
    Active,
    Failed,
}

/// The current epoch-scoped Acquire projection for a controller.
#[cfg(feature = "client")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlAcquireStatus {
    pub epoch: u64,
    pub controller_id: ControllerId,
    pub request_id: Option<ControlRequestId>,
    pub state: ControlAcquireState,
}

#[cfg(feature = "client")]
impl ControlAcquireStatus {
    pub fn retryable(self) -> bool {
        self.state == ControlAcquireState::Failed
    }
}

/// Explicit retry rejection. These errors never send an additional wire request.
#[cfg(feature = "client")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlRetryError {
    StaleSession,
    NotFailed,
    InProgress,
    Active,
    Full,
    Initiator(ControlInitiatorError),
}

#[cfg(feature = "client")]
use control_initiator::ControlInitiatorBook;
#[cfg(feature = "client")]
use log::{debug, info, warn};
#[cfg(feature = "client")]
use roundo_contracts::{
    ClientGameMessage, ControllerId, ResourceCatalogFingerprint, ServerGameMessage,
    ServerResourceMessage,
    control_handoff::{ControlDeadline, ControlRequest, ControlRequestId, ControlRequestKind},
};
#[cfg(feature = "client")]
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateClientCommand, LocalCoordinateClientIpc,
};
#[cfg(feature = "client")]
use roundo_marionette::{
    ClientControlOutcome, ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteIpc,
};
#[cfg(feature = "client")]
use roundo_networking::{ClientHooks, ClientNetwork, NetworkError};
#[cfg(feature = "client")]
use roundo_presence::{ClientPresenceCommand, ClientPresenceIpc};
#[cfg(feature = "client")]
use roundo_toolbox::{BridgeStep, run_polling_bridge};
#[cfg(feature = "client")]
use std::sync::{Arc, atomic::AtomicBool};

/// Process status transition produced by the production client hook.
#[cfg(feature = "client")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientConnectionEvent {
    Established,
    Lost,
    Error(String),
}

/// Projects server networking callbacks into ECS queues.
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "client-test")]
pub use server::TestControlIngress;
#[cfg(feature = "server")]
pub use server::{ServerHooksAdapter, ServerNetworkRuntime, ServerSessionProvider};
