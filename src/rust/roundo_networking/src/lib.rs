//! Business sessions, independent admission queues and priority batching above
//! roundo_quinnet, which transports opaque bytes and owns QUIC/TLS/framing.
//! Each application batch can mix priority lanes and split messages; both peers
//! reconstruct lane-local messages before dispatching business callbacks.

mod batch;
mod service;
mod tls;
mod transport;

pub use service::{
    CertificatePolicy, ClientHooks, ClientNetwork, ClientNetworkConfig, HookFuture, NetworkError,
    OutboundQueuePolicy, PublicSession, ServerAddresses, ServerHooks, ServerNetwork,
    ServerNetworkConfig, ServerSendError, probe_quic_endpoint,
};
