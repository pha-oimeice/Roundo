//! Compatibility re-export of the transport-independent wire contracts.
//!
//! New domain modules should depend on `roundo_contracts` directly. The
//! networking facade keeps this module so host adapters do not need churn.

pub use roundo_contracts::*;
