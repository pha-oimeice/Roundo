#![warn(missing_docs)]

//! Roundo's QUIC transport layer: connections, channels, TLS and byte framing.
//!
//! Forked from bevy_quinnet 0.22.0: <https://github.com/Henauxg/bevy_quinnet>.
//! Game protocols and business sessions belong to the calling layer.

/// Client features
#[cfg(feature = "client")]
pub mod client;
/// Server features
#[cfg(feature = "server")]
pub mod server;
/// Shared features between client & server
pub mod shared;
