//! Shared runtime bridges, transport primitives, and version utilities.
pub mod bridge_runtime;
pub mod fs;
pub mod macros;
pub mod request_response_pipe;
mod templates;
mod traits;

#[allow(unused_imports)]
pub use self::{
    bridge_runtime::{BridgeStep, BridgeThreadGroup, run_polling_bridge},
    templates::*,
    traits::*,
};
