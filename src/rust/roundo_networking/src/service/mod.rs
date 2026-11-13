//! Business sessions and bounded admission above opaque Quinnet byte transfers.

mod client;
mod registry;
mod server;
pub use client::ClientNetwork;
pub use server::{ServerNetwork, ServerSendError};

use crate::{
    batch::{BatchReader, BatchWriter},
    tls,
    transport::{self, ByteTransport},
};
use bevy::prelude::*;
use roundo_quinnet::{
    client::{
        ClientConnectionConfiguration, ClientConnectionConfigurationDefaultables, QuinnetClient,
        QuinnetClientPlugin,
        certificate::{CertStore, CertTrustUpdateEvent},
        connection::{ClientAddrConfiguration, ConnectionState},
    },
    shared::error::{AsyncChannelError, ConnectionSendError, TransportFailure},
};
use roundo_contracts::{
    ClientGameMessage, ClientMessage, ClientResourceMessage, ConnectionId,
    ResourceCatalogFingerprint, ServerGameMessage, ServerMessage, ServerResourceMessage, SessionId,
    TrafficClass, UserSession,
};
use roundo_toolbox::{BridgeStep, run_polling_bridge};
use std::{
    collections::{HashMap, HashSet},
    error::Error as StdError,
    fmt::{Display, Formatter},
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
enum NetworkErrorKind {
    Other,
    Transport(Option<TransportFailure>),
}

#[derive(Clone, Debug)]
pub struct NetworkError {
    message: String,
    kind: NetworkErrorKind,
}
impl NetworkError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: NetworkErrorKind::Other,
        }
    }
    pub(crate) fn from_display(error: impl Display) -> Self {
        Self::new(error.to_string())
    }
    pub(crate) fn transport_message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: NetworkErrorKind::Transport(None),
        }
    }
    pub(crate) fn from_transport(reason: TransportFailure) -> Self {
        Self {
            message: reason.to_string(),
            kind: NetworkErrorKind::Transport(Some(reason)),
        }
    }
    pub(crate) fn transport_end(reason: Option<TransportFailure>, missing: &str) -> Self {
        match reason {
            Some(reason) => Self::from_transport(reason),
            None => Self::transport_message(missing),
        }
    }
    pub(crate) fn is_transport(&self) -> bool {
        matches!(self.kind, NetworkErrorKind::Transport(_))
    }
    /// Returns the original transport cause when one was supplied by the transport.
    pub fn transport_failure(&self) -> Option<&TransportFailure> {
        match &self.kind {
            NetworkErrorKind::Transport(reason) => reason.as_ref(),
            NetworkErrorKind::Other => None,
        }
    }
}
impl Display for NetworkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl StdError for NetworkError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.transport_failure().map(|reason| reason as &dyn StdError)
    }
}

fn log_transport_end(peer: &str, error: &NetworkError) {
    match error.transport_failure() {
        Some(reason) if reason.is_local_close() => {
            log::debug!("transport closed locally: {peer}, reason={error}");
        }
        Some(reason) if reason.is_peer_application_close() => {
            // Preserve the peer's code/reason without claiming that its
            // application operation succeeded or interpreting unknown codes.
            log::info!("transport closed by peer application: {peer}, reason={error}");
        }
        _ => log::warn!("transport failure: {peer}, error={error}"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CertificatePolicy {
    SystemRoots,
    TrustOnFirstUse,
    Insecure,
}

/// Business queue capacities. These do not configure byte batches or the
/// independent 64 KiB submitted-transfer window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutboundQueuePolicy {
    pub game_capacity: usize,
    pub resource_capacity: usize,
}
impl OutboundQueuePolicy {
    fn validate(self) -> Result<(), NetworkError> {
        if self.game_capacity == 0 || self.resource_capacity == 0 {
            Err(NetworkError::new(
                "business outbound queue capacities must be positive",
            ))
        } else {
            Ok(())
        }
    }
}
impl Default for OutboundQueuePolicy {
    fn default() -> Self {
        Self {
            game_capacity: 1024,
            resource_capacity: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionResult {
    Queued,
    Saturated,
    Closed,
}
pub(crate) fn admit<T>(sender: &mpsc::Sender<T>, message: T) -> AdmissionResult {
    match sender.try_send(message) {
        Ok(()) => AdmissionResult::Queued,
        Err(mpsc::error::TrySendError::Full(_)) => AdmissionResult::Saturated,
        Err(mpsc::error::TrySendError::Closed(_)) => AdmissionResult::Closed,
    }
}

#[derive(Clone, Debug)]
pub struct ServerNetworkConfig {
    pub quic_address: SocketAddr,
    pub certificate_directory: PathBuf,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
    pub outbound: OutboundQueuePolicy,
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
    pub outbound: OutboundQueuePolicy,
}
#[derive(Clone, Debug)]
pub struct PublicSession {
    pub user_session: UserSession,
    pub session_name: Option<String>,
}
pub type HookFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'static>>;

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

struct Worker {
    stop: Arc<AtomicBool>,
    join: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}
impl Worker {
    fn start<T: Send + 'static>(
        run: impl FnOnce(
            Arc<AtomicBool>,
            SyncSender<Result<T, NetworkError>>,
        ) -> Result<(), NetworkError>
        + Send
        + 'static,
    ) -> Result<(Self, T), NetworkError> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (ready, receiver) = sync_channel(1);
        let join = thread::spawn(move || {
            if let Err(error) = run(worker_stop, ready.clone()) {
                let _ = ready.try_send(Err(error.clone()));
                log::error!("Quinnet worker stopped: {error}");
            }
        });
        let worker = Self {
            stop,
            join: std::sync::Mutex::new(Some(join)),
        };
        match receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(value)) => Ok((worker, value)),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(NetworkError::from_display(error)),
        }
    }
    fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.lock().unwrap_or_else(|p| p.into_inner()).take() {
            if join.thread().id() == thread::current().id() {
                return;
            }
            if let Err(error) = join.join() {
                log::error!("Quinnet worker panicked: {error:?}");
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn client_configuration(
    address: SocketAddr,
    name: &str,
    policy: &CertificatePolicy,
    trusted: &CertStore,
) -> ClientConnectionConfiguration {
    let bind = match address.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    ClientConnectionConfiguration {
        addr_config: ClientAddrConfiguration::from_addrs_with_name(
            address,
            name.to_owned(),
            SocketAddr::new(bind, 0),
        ),
        cert_mode: tls::client_mode(policy, trusted),
        defaultables: ClientConnectionConfigurationDefaultables {
            send_channels_cfg: transport::channels(),
        },
    }
}
fn update_trust(app: &mut App, trusted: &mut CertStore) {
    for event in app
        .world_mut()
        .resource_mut::<Messages<CertTrustUpdateEvent>>()
        .drain()
    {
        trusted.insert(event.cert_info.server_name, event.cert_info.fingerprint);
    }
}
fn client_send_result(
    result: Result<(), roundo_quinnet::client::ClientSendError>,
) -> Result<bool, NetworkError> {
    match result {
        Ok(()) => Ok(true),
        Err(roundo_quinnet::client::ClientSendError::ConnectionSendError(
            ConnectionSendError::ChannelSendError(AsyncChannelError::FullQueue),
        )) => Ok(false),
        Err(roundo_quinnet::client::ClientSendError::ConnectionSendError(
            ConnectionSendError::ConnectionTerminated(reason),
        )) => Err(NetworkError::from_transport(reason)),
        Err(error) => Err(NetworkError::transport_message(error.to_string())),
    }
}
fn server_send_result(
    result: Result<(), roundo_quinnet::server::ServerSendError>,
) -> Result<bool, NetworkError> {
    match result {
        Ok(()) => Ok(true),
        Err(roundo_quinnet::server::ServerSendError::ConnectionSendError(
            ConnectionSendError::ChannelSendError(AsyncChannelError::FullQueue),
        )) => Ok(false),
        Err(roundo_quinnet::server::ServerSendError::ConnectionSendError(
            ConnectionSendError::ConnectionTerminated(reason),
        )) => Err(NetworkError::from_transport(reason)),
        Err(error) => Err(NetworkError::transport_message(error.to_string())),
    }
}

/// Physical Quinnet reachability only; no business session or batch is created.
pub fn probe_quic_endpoint(
    address: SocketAddr,
    server_name: &str,
    certificate_policy: CertificatePolicy,
    timeout: Duration,
) -> Result<Duration, NetworkError> {
    let started = Instant::now();
    let mut app = App::new();
    app.add_plugins(QuinnetClientPlugin::default());
    let id = app
        .world_mut()
        .resource_mut::<QuinnetClient>()
        .open_connection(client_configuration(
            address,
            server_name,
            &certificate_policy,
            &CertStore::new(),
        ))
        .map_err(NetworkError::from_display)?;
    let stop = AtomicBool::new(false);
    let mut result = None;
    run_polling_bridge(&stop, || {
        app.update();
        let state = app
            .world()
            .resource::<QuinnetClient>()
            .get_connection_by_id(id)
            .map(|c| (c.state(), c.termination_reason()));
        if matches!(state, Some((ConnectionState::Connected, _))) {
            result = Some(Ok(started.elapsed()));
        } else if let Some((ConnectionState::Disconnected, reason)) = state {
            result = Some(Err(NetworkError::transport_end(
                reason,
                "Quinnet probe connection failed without a transport termination cause",
            )));
        } else if started.elapsed() >= timeout {
            result = Some(Err(NetworkError::new("Quinnet probe timed out")));
        }
        if result.is_some() {
            BridgeStep::Stop
        } else {
            BridgeStep::Idle
        }
    });
    app.world_mut()
        .resource_mut::<QuinnetClient>()
        .close_all_connections();
    result.expect("probe loop exits only with a result")
}
