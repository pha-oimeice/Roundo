//! Shared runtime bridges, data structures, configuration, and version utilities.

pub mod bridge_runtime;
mod config_serialization;
pub mod fs;
pub mod macros;
pub mod request_response_pipe;
mod templates;
mod traits;

#[allow(unused_imports)]
pub use self::{
    bridge_runtime::{BridgeStep, BridgeThreadGroup, run_polling_bridge},
    config_serialization::load_or_create_config,
    templates::*,
    traits::*,
};

/// Failure value that can carry caller-usable recovery data.
pub struct ErrorWithData<T> {
    /// Optional fallback or partially recovered value.
    pub data: Option<T>,
    /// Static human-readable failure context; this is not a stable error code.
    pub err_msg: &'static str,
}
