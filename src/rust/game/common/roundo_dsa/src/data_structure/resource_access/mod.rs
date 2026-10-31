//! Unique RAII data ownership, independent runtime grants and synchronous access.
//!
//! The default interface is single-threaded (`!Send + !Sync`). The [`Shared`]
//! storage mode accepts `Send + Sync` payloads for threaded hosts. Both grant
//! access to an entire typed resource, not fields or caller-defined ranges.
//! [`Owner`] is the trusted issuing endpoint. Recipients receive only typed
//! tokens and a [`ProtectedScope`]; neither can issue grants or extend payload
//! lifetime. Cloning a token redistributes the same grant, including revocation.
//!
//! All payload access is confined to a synchronous, non-reentrant `protect`
//! callback. No guard, borrowed payload or modification evidence can escape it
//! into a future. Permission changes require exclusive Owner access, so they
//! cannot cross an active scope. Other resources may be used within a callback.
//!
//! This is not a native-code sandbox: interior mutability, copied data, shared
//! payload handles and callback side effects remain the domain's responsibility.
//! Modification evidence confers no payload getter, generic setter or rollback.
//! Recoverable module errors do not partially publish state; allocation abort,
//! forgotten owners, process termination and destructor panic are not recovery
//! or durability guarantees.
//!

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod access;
mod identity;
mod manager;
mod owner;
mod storage;

pub use storage::{Local, Shared, Storage};

pub use access::{ModifyAccess, ModifyPermission, ProtectedScope, ReadAccess};
pub use identity::{GrantId, ResourceId};
pub use manager::{
    CreateError, Limits, ResourceAccessManager, ResourceMonitor, ResourceStatistics,
};
pub use owner::Owner;

/// Rejection of a lifecycle, issuing or protected access operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessError {
    /// The configured number of live resources has been reached.
    ResourceCapacity,
    /// Grant metadata (including retained revoked IDs/tokens) reached its limit.
    GrantCapacity,
    /// A monotonically issued resource or grant sequence is exhausted.
    IdentityExhausted,
    /// The registry could not reserve storage; no instance was published.
    RegistryAllocation,
    /// The resource belongs to another manager domain or another exact instance.
    ResourceMismatch,
    /// The unique owner has ended; all grants are permanently unusable.
    ResourceEnded,
    /// This grant was revoked; issuing a new grant does not reactivate it.
    GrantRevoked,
    /// A callback attempted to enter an already protected resource again.
    OperationInProgress,
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AccessError {}
