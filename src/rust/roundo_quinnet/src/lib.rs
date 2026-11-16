//! QUIC endpoints and ordered, size-bounded byte messages. No game or ECS types.
//!
//! Callers own the Tokio runtime and TLS configuration. Each connection lazily
//! opens one unidirectional stream per direction; messages use a big-endian u32
//! length prefix. No identities are assigned: `peer_addr` is the current remote
//! socket address, not an authentication credential or a persistent identity.

mod connection;
mod endpoint;
mod error;

pub use connection::{Connection, MessageLimit};
pub use endpoint::{Client, Server};
pub use error::Error;
pub use quinn::{ClientConfig, ConnectionError, ServerConfig, TransportConfig};
