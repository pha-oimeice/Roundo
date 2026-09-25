//! Threaded client/server networking service and transport policies.

mod client;
mod quic;
mod registry;
mod server;

pub use client::ClientNetwork;
pub use server::ServerNetwork;

use crate::connection::{ConnectionIo, ConnectionIoEvent};
use crate::protocol::{
    ClientGameMessage, ClientMessage, ClientResourceMessage, ConnectionId,
    ResourceCatalogFingerprint, ServerGameMessage, ServerMessage, ServerResourceMessage, SessionId,
    StreamId, UserSession,
};
use crate::session::{
    ClientGameSession, ClientResourceSession, ClientSession, ServerResourceSession, ServerSession,
};
use crate::tls;
use std::collections::{HashMap, HashSet};
use std::error::Error as StdError;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};

// Startup notifications are bounded so constructors cannot block indefinitely.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
/// Contextual service failure suitable for logs and public API returns.
pub struct NetworkError {
    message: String,
}

impl NetworkError {
    /// Creates an error owning its human-readable context.
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn from_display(error: impl Display) -> Self {
        Self::new(error.to_string())
    }
}

impl Display for NetworkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for NetworkError {}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Client policy for authenticating the remote certificate.
pub enum CertificatePolicy {
    SystemRoots,
    TrustOnFirstUse,
    Insecure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Bounded queue capacities for prioritized application streams.
pub struct TransportAdmissionPolicy {
    pub stream0_capacity: usize,
    pub stream1_capacity: usize,
}

impl TransportAdmissionPolicy {
    pub const fn capacity(self, stream: StreamId) -> usize {
        match stream {
            StreamId::Stream0 => self.stream0_capacity,
            StreamId::Stream1 => self.stream1_capacity,
        }
    }
}

impl Default for TransportAdmissionPolicy {
    fn default() -> Self {
        Self {
            // Control traffic receives the larger reserve and QUIC priority.
            stream0_capacity: 1024,
            stream1_capacity: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Outcome of attempting non-blocking queue admission.
pub(crate) enum AdmissionResult {
    Queued,
    Saturated,
    Closed,
}

/// Maps Tokio channel outcomes to transport-level admission state.
pub(crate) fn admit<T>(sender: &mpsc::Sender<T>, message: T) -> AdmissionResult {
    match sender.try_send(message) {
        Ok(()) => AdmissionResult::Queued,
        Err(mpsc::error::TrySendError::Full(_)) => AdmissionResult::Saturated,
        Err(mpsc::error::TrySendError::Closed(_)) => AdmissionResult::Closed,
    }
}

#[derive(Clone, Debug)]
/// Inputs required to start the QUIC server runtime.
pub struct ServerNetworkConfig {
    pub quic_address: SocketAddr,
    pub certificate_directory: PathBuf,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
    pub admission: TransportAdmissionPolicy,
}

#[derive(Clone, Copy, Debug)]
pub struct ServerAddresses {
    pub quic_address: SocketAddr,
}

#[derive(Clone, Debug)]
/// Inputs required to start a QUIC client runtime.
pub struct ClientNetworkConfig {
    pub quic_address: SocketAddr,
    pub server_name: String,
    pub certificate_policy: CertificatePolicy,
    pub reconnect_delay: Duration,
    pub admission: TransportAdmissionPolicy,
}

#[derive(Clone, Debug)]
/// Server-confirmed public game-session identity.
pub struct PublicSession {
    pub user_session: UserSession,
    pub session_name: Option<String>,
}

pub type HookFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'static>>;

/// Host callbacks invoked by authenticated server connections.
pub trait ServerHooks: Send + Sync + 'static {
    fn initialize(&self) -> HookFuture<()>;

    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        ResourceCatalogFingerprint::default()
    }

    fn public_session(&self) -> HookFuture<PublicSession>;

    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool>;

    fn on_session_connected(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        peer_address: SocketAddr,
    );

    fn on_session_disconnected(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        peer_address: SocketAddr,
    );

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientGameMessage,
    );

    fn on_client_resource_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientResourceMessage,
    );
}

/// Host callbacks invoked by the client networking runtime.
pub trait ClientHooks: Send + Sync + 'static {
    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        ResourceCatalogFingerprint::default()
    }

    fn on_server_game_message(&self, message: ServerGameMessage);

    fn on_server_resource_message(&self, message: ServerResourceMessage);

    fn on_connection_established(&self) {}

    fn on_connection_lost(&self) {}

    fn on_connection_error(&self, _: &NetworkError) {}
}

#[cfg(test)]
mod admission_tests {
    use super::{AdmissionResult, admit};

    #[test]
    fn bounded_admission_distinguishes_saturation_from_shutdown() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        assert_eq!(admit(&sender, 1), AdmissionResult::Queued);
        assert_eq!(admit(&sender, 2), AdmissionResult::Saturated);
        drop(receiver);
        assert_eq!(admit(&sender, 3), AdmissionResult::Closed);
    }
}

/// Performs a bounded QUIC reachability probe without joining a session.
pub fn probe_quic_endpoint(
    address: SocketAddr,
    server_name: &str,
    certificate_policy: CertificatePolicy,
    timeout: Duration,
) -> Result<Duration, NetworkError> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(NetworkError::from_display)?;
    runtime.block_on(async move {
        let started = std::time::Instant::now();
        tokio::time::timeout(timeout, async {
            let tls_config = tls::create_client_tls_config(&certificate_policy)?;
            let endpoint = quic::client_endpoint(address, tls_config)?;
            let connection = quic::dial(&endpoint, address, server_name).await?;
            let () = connection.close(quinn::VarInt::from_u32(0), b"probe complete");
            endpoint.wait_idle().await;
            Ok::<(), NetworkError>(())
        })
        .await
        .map_err(|_| NetworkError::new("QUIC probe timed out"))??;
        Ok(started.elapsed())
    })
}
