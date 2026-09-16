//! Shared runtime bridges, data structures, configuration, and version utilities.

pub mod bridge_runtime;
mod config_serialization;
pub mod fs;
pub mod macros;
pub mod request_response_pipe;
mod templates;
mod traits;
pub mod version;

#[allow(unused_imports)]
pub use self::{
    bridge_runtime::{BridgeStep, BridgeThreadGroup, run_polling_bridge},
    config_serialization::load_or_create_config,
    templates::*,
    traits::*,
    version::UpdateVersion,
};
use std::net::{SocketAddr, ToSocketAddrs};

/// Failure value that can carry caller-usable recovery data.
pub struct ErrorWithData<T> {
    /// Optional fallback or partially recovered value.
    pub data: Option<T>,
    /// Static human-readable failure context; this is not a stable error code.
    pub err_msg: &'static str,
}

/// Resolves and returns the first socket address produced for `host:port` text.
///
/// This may synchronously perform name resolution. Multiple results are not
/// sorted or retried, so selection follows the resolver's order.
///
/// # Panics
///
/// Panics if resolution fails or yields no address.
#[inline(always)]
pub fn string_to_socket_addr(addr: &String) -> SocketAddr {
    addr.to_socket_addrs()
        .unwrap_or_else(|e| panic!("Failed to resolve address: {}\n{}", addr, e))
        .next()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::AddrNotAvailable))
        .unwrap_or_else(|e| panic!("Failed to get socket address: {}", e))
}

/// Initializes the process-global logger with build-dependent Roundo filters.
///
/// # Panics
///
/// Panics if a global logger was already installed or initialization otherwise fails.
pub fn init_logger() {
    let filter_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    env_logger::Builder::new()
        .default_format()
        .format_level(true)
        .filter_module("marionette", filter_level)
        .filter_module("roundo", filter_level)
        .write_style(env_logger::WriteStyle::Always)
        .init();
}
