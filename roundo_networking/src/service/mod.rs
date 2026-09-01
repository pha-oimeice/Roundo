mod client;
mod quic;
mod registry;
mod server;

pub use client::ClientNetwork;
pub use server::ServerNetwork;

use crate::connection::{ConnectionIo, ConnectionIoEvent};
use crate::protocol::{
    ClientGameMessage, ClientMessage, ClientResourceMessage, ConnectionId, ServerGameMessage,
    ServerMessage, ServerResourceMessage, SessionId, StreamId, UserSession,
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
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct NetworkError {
    message: String,
}

impl NetworkError {
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
pub enum CertificatePolicy {
    SystemRoots,
    TrustOnFirstUse,
    Insecure,
}

#[derive(Clone, Debug)]
pub struct ServerNetworkConfig {
    pub quic_address: SocketAddr,
    pub certificate_directory: PathBuf,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ServerAddresses {
    pub quic_address: SocketAddr,
}

#[derive(Clone, Debug)]
pub struct ClientNetworkConfig {
    pub quic_address: SocketAddr,
    pub server_name: String,
    pub certificate_policy: CertificatePolicy,
    pub reconnect_delay: Duration,
}

#[derive(Clone, Debug)]
pub struct PublicSession {
    pub user_session: UserSession,
    pub session_name: Option<String>,
}

pub type HookFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'static>>;

pub trait ServerHooks: Send + Sync + 'static {
    fn initialize(&self) -> HookFuture<()>;

    fn public_session(&self) -> HookFuture<PublicSession>;

    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool>;

    fn on_session_connected(&self, connection_id: ConnectionId, user_session: UserSession);

    fn on_session_disconnected(&self, connection_id: ConnectionId, user_session: UserSession);

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

pub trait ClientHooks: Send + Sync + 'static {
    fn on_server_game_message(&self, message: ServerGameMessage);

    fn on_server_resource_message(&self, message: ServerResourceMessage);

    fn on_connection_established(&self) {}

    fn on_connection_lost(&self) {}

    fn on_connection_error(&self, _: &NetworkError) {}
}

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
            let connection = quic::connect(&endpoint, address, server_name).await?;
            connection.close(quinn::VarInt::from_u32(0), b"probe complete");
            endpoint.wait_idle().await;
            Ok::<(), NetworkError>(())
        })
        .await
        .map_err(|_| NetworkError::new("QUIC probe timed out"))??;
        Ok(started.elapsed())
    })
}
