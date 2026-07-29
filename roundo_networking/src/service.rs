use crate::connection::run_writer;
use crate::protocol::{
    ClientGameMessage, ClientMessage, ConnectionId, ConnectionToken, ServerGameMessage,
    ServerMessage, SessionId, UserSession,
};
use crate::session::{ClientGameSession, ClientSession, ServerSession};
use crate::tls::{self, ClientTlsStream, TlsIncoming, TlsServer};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};

const CONNECTION_TOKEN_TTL: Duration = Duration::from_secs(60);
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
    pub game_address: SocketAddr,
    pub public_address: SocketAddr,
    pub certificate_directory: PathBuf,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ServerAddresses {
    pub game_address: SocketAddr,
    pub public_address: SocketAddr,
}

#[derive(Clone, Debug)]
pub struct ClientNetworkConfig {
    pub game_address: SocketAddr,
    pub public_address: SocketAddr,
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

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientGameMessage,
    );
}

pub trait ClientHooks: Send + Sync + 'static {
    fn on_server_game_message(&self, message: ServerGameMessage);

    fn on_connection_error(&self, _: &NetworkError) {}
}

pub struct ServerNetwork {
    registry: ConnectionRegistry,
    shutdown: watch::Sender<bool>,
    addresses: ServerAddresses,
}

impl ServerNetwork {
    pub fn start(
        config: ServerNetworkConfig,
        hooks: Arc<dyn ServerHooks>,
    ) -> Result<Self, NetworkError> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(NetworkError::from_display)?;
        let registry = ConnectionRegistry::default();
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (ready_sender, ready_receiver) = sync_channel(1);
        let runtime_registry = registry.clone();
        let join_handle = thread::spawn(move || {
            if let Err(error) = runtime.block_on(run_server(
                config,
                hooks,
                runtime_registry,
                shutdown_receiver,
                ready_sender,
            )) {
                log::error!("network server stopped: {error}");
            }
        });

        let addresses = match ready_receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(addresses)) => addresses,
            Ok(Err(error)) => {
                let _ = join_handle.join();
                return Err(error);
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = shutdown.send(true);
                let _ = join_handle.join();
                return Err(NetworkError::new("timed out while starting network server"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = join_handle.join();
                return Err(NetworkError::new("network server stopped before startup"));
            }
        };

        Ok(Self {
            registry,
            shutdown,
            addresses,
        })
    }

    pub const fn addresses(&self) -> ServerAddresses {
        self.addresses
    }

    pub fn send_to_connection(
        &self,
        connection_id: ConnectionId,
        message: ServerGameMessage,
    ) -> bool {
        self.registry.send_to_connection(connection_id, message)
    }

    pub fn send_to_session(&self, user_session: UserSession, message: ServerGameMessage) {
        self.registry.send_to_session(user_session, message);
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Drop for ServerNetwork {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

pub struct ClientNetwork {
    outbound: mpsc::UnboundedSender<ClientGameMessage>,
    shutdown: watch::Sender<bool>,
}

impl ClientNetwork {
    pub fn start(
        config: ClientNetworkConfig,
        hooks: Arc<dyn ClientHooks>,
    ) -> Result<Self, NetworkError> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(NetworkError::from_display)?;
        let (outbound, outbound_receiver) = mpsc::unbounded_channel();
        let (shutdown, shutdown_receiver) = watch::channel(false);
        thread::spawn(move || {
            runtime.block_on(run_client(
                config,
                hooks,
                outbound_receiver,
                shutdown_receiver,
            ));
        });

        Ok(Self { outbound, shutdown })
    }

    pub fn send(&self, message: ClientGameMessage) -> Result<(), NetworkError> {
        self.outbound
            .send(message)
            .map_err(|_| NetworkError::new("network client is no longer running"))
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Drop for ClientNetwork {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn run_server(
    config: ServerNetworkConfig,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
    shutdown: watch::Receiver<bool>,
    ready_sender: SyncSender<Result<ServerAddresses, NetworkError>>,
) -> Result<(), NetworkError> {
    if let Err(error) = hooks.initialize().await {
        let error = NetworkError::new(error);
        let _ = ready_sender.send(Err(error.clone()));
        return Err(error);
    }

    let tls = match tls::prepare_server_tls(
        &config.certificate_directory,
        &config.server_alternative_names,
        config.generate_self_signed_certificate,
    ) {
        Ok(tls) => tls,
        Err(error) => {
            let _ = ready_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    let game_listener = match TlsServer::bind(config.game_address, Arc::clone(&tls.config)).await {
        Ok(listener) => listener,
        Err(error) => {
            let error = NetworkError::from_display(error);
            let _ = ready_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    let game_address = match game_listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            let error = NetworkError::from_display(error);
            let _ = ready_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    let state = Arc::new(HttpState {
        hooks: Arc::clone(&hooks),
        tickets: registry.tickets(),
        certificate_pem: tls.certificate_pem,
    });
    let http_config = axum_server::tls_rustls::RustlsConfig::from_config(tls.config);
    let http_handle = axum_server::Handle::new();
    let http_task = tokio::spawn(
        axum_server::bind_rustls(config.public_address, http_config)
            .handle(http_handle.clone())
            .serve(http_router(state).into_make_service()),
    );
    let public_address = match http_handle.listening().await {
        Some(address) => address,
        None => {
            let error = NetworkError::new("failed to bind public HTTPS listener");
            let _ = ready_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    let addresses = ServerAddresses {
        game_address,
        public_address,
    };
    log::debug!(
        "network listeners started: game={}, public={}",
        game_address,
        public_address
    );
    let _ = ready_sender.send(Ok(addresses));

    run_game_listener(game_listener, hooks, registry, shutdown).await;
    http_handle.shutdown();
    match http_task.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => log::warn!("public HTTPS listener stopped: {error}"),
        Err(error) => log::warn!("public HTTPS task stopped: {error}"),
    }
    Ok(())
}

async fn run_game_listener(
    listener: TlsServer,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            incoming = listener.accept() => match incoming {
                Ok(incoming) => {
                    let hooks = Arc::clone(&hooks);
                    let registry = registry.clone();
                    tokio::spawn(async move {
                        process_server_connection(incoming, hooks, registry).await;
                    });
                }
                Err(error) => log::warn!("failed to accept game TCP connection: {error}"),
            }
        }
    }
}

async fn process_server_connection(
    incoming: TlsIncoming,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
) {
    let peer_address = incoming.peer_addr();
    log::debug!("accepted game TCP connection from {peer_address}");
    let stream = match incoming.handshake().await {
        Ok(stream) => stream,
        Err(error) if error.is_peer_disconnect() => {
            log::debug!(
                "game peer {} disconnected before TLS handshake",
                error.peer_addr()
            );
            return;
        }
        Err(error) => {
            log::warn!("rejected TLS handshake from {peer_address}: {error}");
            return;
        }
    };
    log::debug!("TLS handshake completed for game peer {peer_address}");
    let (pending_session, token) = match ServerSession::new(stream).receive_authentication().await {
        Ok(result) => result,
        Err(error) => {
            log::warn!("game peer {peer_address} failed before authentication: {error}");
            return;
        }
    };
    let user_session = match registry.tickets().claim(token.as_str()) {
        Some(session) => session,
        None => {
            let _ = pending_session
                .reject(crate::protocol::ProtocolErrorCode::AuthenticationRejected)
                .await;
            return;
        }
    };
    log::debug!(
        "claimed connection ticket for game peer {peer_address}: user_id={}, session_id={}",
        user_session.user_id.0,
        user_session.session_id.0
    );
    let is_open = match hooks.session_is_open(user_session.session_id).await {
        Ok(is_open) => is_open,
        Err(error) => {
            log::error!("failed to validate game session: {error}");
            false
        }
    };
    if !is_open {
        let _ = pending_session
            .reject(crate::protocol::ProtocolErrorCode::AuthenticationRejected)
            .await;
        return;
    }
    let authenticated = match pending_session
        .confirm(crate::protocol::AuthenticationInfo { user_session })
        .await
    {
        Ok(session) => session,
        Err(error) => {
            log::warn!("failed to confirm game authentication: {error}");
            return;
        }
    };
    let connection = match authenticated.enter_game().await {
        Ok(session) => session.into_connection(),
        Err(error) => {
            log::warn!("game peer {peer_address} failed protocol setup: {error}");
            return;
        }
    };

    let (reader, writer) = connection.into_split();
    let (outbound, outbound_receiver) = mpsc::unbounded_channel();
    let connection_id = registry.register(user_session, outbound.clone());
    log::debug!(
        "game session entered: peer={peer_address}, connection_id={}, user_id={}, session_id={}",
        connection_id.0,
        user_session.user_id.0,
        user_session.session_id.0
    );
    hooks.on_session_connected(connection_id, user_session);
    let writer_task = tokio::spawn(run_writer(writer, outbound_receiver));
    let mut reader = reader;
    while let Ok(ClientMessage::Game(message)) = reader.receive().await {
        hooks.on_client_game_message(connection_id, user_session, message);
    }
    registry.unregister(connection_id, user_session);
    log::debug!(
        "game session closed: peer={peer_address}, connection_id={}",
        connection_id.0
    );
    drop(outbound);
    let _ = writer_task.await;
}

async fn run_client(
    config: ClientNetworkConfig,
    hooks: Arc<dyn ClientHooks>,
    mut outbound: mpsc::UnboundedReceiver<ClientGameMessage>,
    mut shutdown: watch::Receiver<bool>,
) {
    let tls_config = match tls::create_client_tls_config(&config.certificate_policy) {
        Ok(config) => config,
        Err(error) => {
            log::error!("failed to configure TLS client: {error}");
            return;
        }
    };
    loop {
        if *shutdown.borrow() {
            return;
        }
        match establish_client_session(&config, Arc::clone(&tls_config)).await {
            Ok(session) => {
                run_client_session(session, Arc::clone(&hooks), &mut outbound, &mut shutdown).await
            }
            Err(error) => {
                log::warn!("game connection failed: {error}");
                hooks.on_connection_error(&error);
            }
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = tokio::time::sleep(config.reconnect_delay) => {}
        }
    }
}

async fn establish_client_session(
    config: &ClientNetworkConfig,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<ClientGameSession<ClientTlsStream>, NetworkError> {
    let connection_token = request_public_connection_token(config).await?;
    let server_name = rustls::pki_types::ServerName::try_from(config.server_name.clone())
        .map_err(NetworkError::from_display)?;
    let stream = tls::connect(config.game_address, server_name, tls_config)
        .await
        .map_err(NetworkError::from_display)?;
    ClientSession::new(stream)
        .authenticate(ConnectionToken::new(connection_token))
        .await
        .map_err(NetworkError::from_display)?
        .enter_game()
        .await
        .map_err(NetworkError::from_display)
}

async fn run_client_session(
    session: ClientGameSession<ClientTlsStream>,
    hooks: Arc<dyn ClientHooks>,
    outbound: &mut mpsc::UnboundedReceiver<ClientGameMessage>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let (mut reader, mut writer) = session.into_connection().into_split();
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            message = outbound.recv() => match message {
                Some(message) => {
                    if writer.send(&ClientMessage::Game(message)).await.is_err() {
                        return;
                    }
                }
                None => return,
            },
            message = reader.receive() => match message {
                Ok(ServerMessage::Game(message)) => hooks.on_server_game_message(message),
                Ok(ServerMessage::Authenticated { .. } | ServerMessage::Error { .. }) | Err(_) => return,
            },
        }
    }
}

async fn request_public_connection_token(
    config: &ClientNetworkConfig,
) -> Result<String, NetworkError> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(config.certificate_policy != CertificatePolicy::SystemRoots)
        .build()
        .map_err(NetworkError::from_display)?;
    let response = client
        .post(format!(
            "https://{}/public/connection-token",
            config.public_address
        ))
        .send()
        .await
        .map_err(NetworkError::from_display)?
        .error_for_status()
        .map_err(NetworkError::from_display)?
        .json::<ConnectionTokenResponse>()
        .await
        .map_err(NetworkError::from_display)?;
    Ok(response.connection_token)
}

#[derive(Clone, Default)]
struct ConnectionRegistry {
    next_id: Arc<AtomicU64>,
    senders: Arc<RwLock<HashMap<ConnectionId, mpsc::UnboundedSender<ServerMessage>>>>,
    sessions: Arc<RwLock<HashMap<UserSession, HashSet<ConnectionId>>>>,
    tickets: TicketAuthority,
}

impl ConnectionRegistry {
    fn register(
        &self,
        user_session: UserSession,
        sender: mpsc::UnboundedSender<ServerMessage>,
    ) -> ConnectionId {
        let connection_id = ConnectionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.senders
            .write()
            .expect("connection sender registry lock poisoned")
            .insert(connection_id, sender);
        self.sessions
            .write()
            .expect("connection session registry lock poisoned")
            .entry(user_session)
            .or_default()
            .insert(connection_id);
        connection_id
    }

    fn unregister(&self, connection_id: ConnectionId, user_session: UserSession) {
        self.senders
            .write()
            .expect("connection sender registry lock poisoned")
            .remove(&connection_id);
        let mut sessions = self
            .sessions
            .write()
            .expect("connection session registry lock poisoned");
        if let Some(connection_ids) = sessions.get_mut(&user_session) {
            connection_ids.remove(&connection_id);
            if connection_ids.is_empty() {
                sessions.remove(&user_session);
            }
        }
    }

    fn send_to_connection(&self, connection_id: ConnectionId, message: ServerGameMessage) -> bool {
        self.senders
            .read()
            .expect("connection sender registry lock poisoned")
            .get(&connection_id)
            .is_some_and(|sender| sender.send(ServerMessage::Game(message)).is_ok())
    }

    fn send_to_session(&self, user_session: UserSession, message: ServerGameMessage) {
        let connection_ids = self
            .sessions
            .read()
            .expect("connection session registry lock poisoned")
            .get(&user_session)
            .cloned()
            .unwrap_or_default();
        let senders = self
            .senders
            .read()
            .expect("connection sender registry lock poisoned");
        for connection_id in connection_ids {
            if let Some(sender) = senders.get(&connection_id) {
                let _ = sender.send(ServerMessage::Game(message.clone()));
            }
        }
    }

    fn tickets(&self) -> TicketAuthority {
        self.tickets.clone()
    }
}

#[derive(Clone, Default)]
struct TicketAuthority {
    tickets: Arc<RwLock<HashMap<String, IssuedTicket>>>,
}

impl TicketAuthority {
    fn issue(&self, user_session: UserSession) -> ConnectionTokenResponse {
        let expires_at = unix_timestamp() + CONNECTION_TOKEN_TTL.as_secs();
        let token = format!("{:032x}", rand::random::<u128>());
        self.tickets
            .write()
            .expect("connection ticket registry lock poisoned")
            .insert(
                token.clone(),
                IssuedTicket {
                    user_session,
                    expires_at,
                },
            );
        ConnectionTokenResponse {
            connection_token: token,
            expires_at,
        }
    }

    fn claim(&self, token: &str) -> Option<UserSession> {
        let now = unix_timestamp();
        let mut tickets = self
            .tickets
            .write()
            .expect("connection ticket registry lock poisoned");
        tickets.retain(|_, ticket| ticket.expires_at > now);
        tickets
            .remove(token)
            .and_then(|ticket| (ticket.expires_at > now).then_some(ticket.user_session))
    }
}

#[derive(Clone)]
struct IssuedTicket {
    user_session: UserSession,
    expires_at: u64,
}

#[derive(Clone)]
struct HttpState {
    hooks: Arc<dyn ServerHooks>,
    tickets: TicketAuthority,
    certificate_pem: String,
}

fn http_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/", get(get_server_certificate))
        .route("/server-cert", get(get_server_certificate))
        .route(
            "/public/connection-token",
            post(issue_public_connection_token),
        )
        .with_state(state)
}

async fn get_server_certificate(State(state): State<Arc<HttpState>>) -> String {
    log::debug!("serving TLS certificate through public HTTPS endpoint");
    state.certificate_pem.clone()
}

async fn issue_public_connection_token(
    State(state): State<Arc<HttpState>>,
) -> Result<Json<PublicConnectionTokenResponse>, (StatusCode, String)> {
    let public_session = state
        .hooks
        .public_session()
        .await
        .map_err(internal_server_error)?;
    let ticket = state.tickets.issue(public_session.user_session);
    log::debug!(
        "issued connection ticket: user_id={}, session_id={}",
        public_session.user_session.user_id.0,
        public_session.user_session.session_id.0
    );
    Ok(Json(PublicConnectionTokenResponse {
        user_id: public_session.user_session.user_id.0,
        session_id: public_session.user_session.session_id.0,
        session_name: public_session.session_name,
        connection_token: ticket.connection_token,
        expires_at: ticket.expires_at,
    }))
}

fn internal_server_error(error: String) -> (StatusCode, String) {
    log::error!("public network request failed: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, error)
}

#[derive(Deserialize)]
struct ConnectionTokenResponse {
    connection_token: String,
    expires_at: u64,
}

#[derive(Serialize)]
struct PublicConnectionTokenResponse {
    user_id: i32,
    session_id: i32,
    session_name: Option<String>,
    connection_token: String,
    expires_at: u64,
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX_EPOCH")
        .as_secs()
}
