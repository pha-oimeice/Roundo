//! Controller handoff values shared by the production client and server.
//! Prefix observation is distinct from gate activation and release settlement.

use crate::{ConnectionId, ControllerBindingId, ControllerId};
use serde::{Deserialize, Serialize};

/// Globally unique request correlation value allocated outside the wire contract.
/// Never reused within or across sessions; not an input Submit sequence/epoch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ControlRequestId(pub u128);

/// Exclusive deadline in the request owner's nondecreasing logical clock units.
/// Transports must not compare this value against another process's clock.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlDeadline(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ControlRequestKind {
    Acquire,
    Release,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlRequest {
    pub id: ControlRequestId,
    pub controller_id: ControllerId,
    pub kind: ControlRequestKind,
    pub deadline: ControlDeadline,
}

impl ControlRequest {
    /// Structural admission only; the request owner must also reject repeated IDs,
    /// authenticate the connection and use a bounded ticket book for completion.
    pub fn validate(&self, owner_now: u64) -> Result<(), ControlHandoffError> {
        if self.id.0 == 0 {
            return Err(ControlHandoffError::InvalidRequest);
        }
        if self.deadline.0 <= owner_now {
            return Err(ControlHandoffError::DeadlineElapsed);
        }
        Ok(())
    }

    /// Check against owner-retained IDs (pending AND recently terminal). The
    /// caller owns bounded retention and must not admit a replay after eviction.
    pub fn validate_new(
        &self,
        owner_now: u64,
        retained_ids: impl IntoIterator<Item = ControlRequestId>,
    ) -> Result<(), ControlHandoffError> {
        self.validate(owner_now)?;
        if retained_ids.into_iter().any(|id| id == self.id) {
            return Err(ControlHandoffError::DuplicateRequest);
        }
        Ok(())
    }
}

/// Client producer freeze request. Sending this is not proof that a producer stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Freeze {
    pub request_id: ControlRequestId,
    pub controller_id: ControllerId,
}

/// Stream-0 marker sent after the frozen producer's previous submissions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DrainBarrier {
    pub request_id: ControlRequestId,
    pub controller_id: ControllerId,
    /// Client-local epoch for rejecting stale outbound work; not a server identity.
    pub session_epoch: u64,
}

/// ECS-processed prefix acknowledgement, not a network enqueue acknowledgement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PrefixAck {
    pub request_id: ControlRequestId,
    pub connection_id: ConnectionId,
}

/// Acquire-only invitation correlated to the original request. It does not grant control;
/// the client must discard old pending input and prediction before sending Ready.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlPreparing {
    pub request_id: ControlRequestId,
    pub controller_id: ControllerId,
}

/// Client's stream-0 readiness after discarding old prediction and pending input.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Ready {
    pub request_id: ControlRequestId,
    pub controller_id: ControllerId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ControlHandoffError {
    InvalidRequest,
    DuplicateRequest,
    DeadlineElapsed,
    DrainUnproven,
    AccessDenied,
    Busy,
    /// Required transition or settlement work could not complete; never a success.
    NotReady,
}

/// Terminal success must be emitted only after the gate commits; deserializing
/// this value does not authenticate its source or confer authority.
/// Exactly once describes the ECS book's logical decision, not transport delivery:
/// a failed IPC/network send cannot guarantee that a client observes the result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ControlSuccess {
    Released,
    Acquired(ControllerBindingId),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlTerminal {
    pub request_id: ControlRequestId,
    pub controller_id: ControllerId,
    pub outcome: Result<ControlSuccess, ControlHandoffError>,
}
