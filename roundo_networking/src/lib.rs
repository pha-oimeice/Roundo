//! Shared TLS game-networking primitives.
//!
//! TCP and TLS setup end at [`tls`]. Everything after a successful TLS
//! handshake uses [`connection::Connection`]; protocol and session rules stay
//! in this crate so clients and servers cannot drift apart.

pub mod connection;
pub mod echo_test;
pub mod frame;
pub mod protocol;
pub mod schema;
pub mod session;
pub mod tls;

mod service;

mod error;

pub use error::ProtocolError;
pub use protocol::{
    CharacterId, CharacterSnapshot, ClientGameMessage, ConnectionId, ServerGameMessage, SessionId,
    UserId, UserSession,
};
pub use service::{
    CertificatePolicy, ClientHooks, ClientNetwork, ClientNetworkConfig, HookFuture, NetworkError,
    PublicSession, ServerAddresses, ServerHooks, ServerNetwork, ServerNetworkConfig,
};
