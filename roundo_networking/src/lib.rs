//! Shared prioritized QUIC networking implementation.
//!
//! Client and server hosts configure the network, implement hooks, and exchange
//! protocol messages. Runtime ownership, QUIC/TLS, framing, session establishment,
//! unidirectional I/O tasks, reconnects, and connection lifecycle stay inside
//! this crate. Game traffic uses high-priority `stream0`; resource traffic uses
//! low-priority `stream1`.

pub mod connection;
pub mod echo_test;
pub mod frame;
mod protocol;
pub mod schema;
pub mod session;

mod service;
mod tls;

mod error;

pub use error::ProtocolError;
pub use service::{
    CertificatePolicy, ClientHooks, ClientNetwork, ClientNetworkConfig, HookFuture, NetworkError,
    PublicSession, ServerAddresses, ServerHooks, ServerNetwork, ServerNetworkConfig,
    TransportAdmissionPolicy, probe_quic_endpoint,
};
