use std::{
    error::Error,
    net::{AddrParseError, IpAddr, SocketAddr},
    sync::Arc,
};

use bevy::log::{error, info, trace};
use bytes::Bytes;
use quinn::{crypto::rustls::QuicClientConfig, ClientConfig, Endpoint};
use quinn_proto::ConnectionStats;
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::{
    runtime,
    sync::{
        broadcast,
        mpsc::{self, error::TryRecvError},
    },
};

#[cfg(feature = "bincode-messages")]
/// Module for the client's bincode serde helpers feature
pub mod messages;

use crate::shared::{
    channels::{
        tasks::{spawn_recv_channels_tasks, spawn_send_channels_tasks_spawner},
        ChannelAsyncMessage, ChannelConfig, ChannelId, ChannelSyncMessage, CloseReason, CloseRecv,
        CloseSend, SendChannelsConfiguration,
    },
    error::{AsyncChannelError, ChannelCloseError, ChannelCreationError, TransportFailure, TransportOperation},
    peer_connection::{
        ChannelAsyncMsgRecv, ChannelAsyncMsgSend, ChannelSyncMsgRecv, ChannelSyncMsgSend,
        ChannelsIdsPool, ConnectionTermination, PayloadRecv, PayloadSend, PeerConnection,
    },
    InternalConnectionRef, DEFAULT_INTERNAL_MESSAGES_CHANNEL_SIZE,
    DEFAULT_KILL_MESSAGE_QUEUE_SIZE, DEFAULT_MESSAGE_QUEUE_SIZE,
    DEFAULT_QCHANNEL_MESSAGES_CHANNEL_SIZE,
};

use super::{
    certificate::{
        load_known_hosts_store_from_config, CertificateVerificationMode, SkipServerVerification,
        TofuServerVerification,
    },
    error::{ClientPayloadSendError, ClientSendError},
    ClientAsyncMessage, ClientConnectionCloseError,
};

/// Alias type for a local id of a connection
pub type ConnectionLocalId = u64;

/// Connection event raised when the client just connected to the server. Raised in the CoreStage::PreUpdate stage.
#[derive(bevy::ecs::message::Message, Debug, Copy, Clone)]
pub struct ConnectionEvent {
    /// Local id of the connection
    pub id: ConnectionLocalId,
}

/// Connection event raised when the client failed to connect to the server. Raised in the CoreStage::PreUpdate stage.
#[derive(bevy::ecs::message::Message, Debug, Clone)]
pub struct ConnectionFailedEvent {
    /// Local id of the connection which failed to connect
    pub id: ConnectionLocalId,
    /// Error raised during the connection
    pub err: quinn::ConnectionError,
}

/// ConnectionLost event raised when the client is considered disconnected from the server. Raised in the CoreStage::PreUpdate stage.
#[derive(bevy::ecs::message::Message, Debug, Clone)]
pub struct ConnectionLostEvent {
    /// Local id of the connection
    pub id: ConnectionLocalId,
    /// Primary terminal transport cause, including the original QUIC error.
    pub reason: TransportFailure,
}

/// Configuration of a client connection, used when connecting to a server
#[derive(Debug, Clone)]
pub struct ClientAddrConfiguration {
    /// Address and port of the server to connect to.
    pub server_addr: SocketAddr,
    /// Server hostname for certificate verification, can be just the server IP.
    pub server_hostname: String,
    /// Local address and port to bind to.
    pub local_bind_addr: SocketAddr,
}

impl ClientAddrConfiguration {
    /// Creates a new ClientAddrConfiguration
    ///
    /// # Arguments
    ///
    /// * `server_addr_str` - IP address and port of the server
    /// * `local_bind_addr_str` - Local address and port to bind to separated by `:`. The address should usually be a wildcard like `0.0.0.0` (for an IPv4) or `[::]` (for an IPv6), which allow communication with any reachable IPv4 or IPv6 address. See [`std::net::SocketAddrV4`] and [`std::net::SocketAddrV6`] or [`quinn::Endpoint`] for more precision. For the local port to bind to, use 0 to get an OS-assigned port.
    ///
    /// # Examples
    ///
    /// Connect to an IPv4 server hosted on localhost (127.0.0.1), which is listening on port 6000. Use 0 as a local bind port to let the OS assign a port.
    /// ```
    /// use roundo_quinnet::client::connection::ClientAddrConfiguration;
    /// let config = ClientAddrConfiguration::from_strings(
    ///                 "127.0.0.1:6000",
    ///                 "0.0.0.0:0"
    ///             );
    /// ```
    /// Connect to an IPv6 server hosted on localhost (::1), which is listening on port 6000. Use 0 as a local bind port to let the OS assign a port.
    /// ```
    /// use roundo_quinnet::client::connection::ClientAddrConfiguration;
    /// let config = ClientAddrConfiguration::from_strings(
    ///                 "[::1]:6000",
    ///                 "[::]:0"
    ///             );
    /// ```
    pub fn from_strings(
        server_addr_str: &str,
        local_bind_addr_str: &str,
    ) -> Result<Self, AddrParseError> {
        let server_addr = server_addr_str.parse()?;
        let local_bind_addr = local_bind_addr_str.parse()?;
        Ok(Self::from_addrs(server_addr, local_bind_addr))
    }

    /// Same as [`ClientAddrConfiguration::from_strings`], but with an additional `server_hostname` for certificate verification if it is not just the server IP.
    pub fn from_strings_with_name(
        server_addr_str: &str,
        server_hostname: String,
        local_bind_addr_str: &str,
    ) -> Result<Self, AddrParseError> {
        Ok(Self::from_addrs_with_name(
            server_addr_str.parse()?,
            server_hostname,
            local_bind_addr_str.parse()?,
        ))
    }

    /// Creates a new ClientAddrConfiguration
    ///
    /// # Arguments
    ///
    /// * `server_ip` - IP address of the server
    /// * `server_port` - Port of the server
    /// * `local_bind_ip` - Local IP address to bind to. The address should usually be a wildcard like `0.0.0.0` (for an IPv4) or `0:0:0:0:0:0:0:0` (for an IPv6), which allow communication with any reachable IPv4 or IPv6 address. See [`std::net::Ipv4Addr`] and [`std::net::Ipv6Addr`] for more precision.
    /// * `local_bind_port` - Local port to bind to. Use 0 to get an OS-assigned port.
    ///
    /// # Examples
    ///
    /// Connect to an IPv4 server hosted on localhost (127.0.0.1), which is listening on port 6000. Use 0 as a local bind port to let the OS assign a port.
    /// ```
    /// use std::net::Ipv6Addr;
    /// use roundo_quinnet::client::connection::ClientAddrConfiguration;
    /// let config = ClientAddrConfiguration::from_ips(
    ///                 Ipv6Addr::LOCALHOST,
    ///                 6000,
    ///                 Ipv6Addr::UNSPECIFIED,
    ///                 0
    ///             );
    /// ```
    pub fn from_ips(
        server_ip: impl Into<IpAddr>,
        server_port: u16,
        local_bind_ip: impl Into<IpAddr>,
        local_bind_port: u16,
    ) -> Self {
        Self::from_addrs(
            SocketAddr::new(server_ip.into(), server_port),
            SocketAddr::new(local_bind_ip.into(), local_bind_port),
        )
    }

    /// Same as [`ClientAddrConfiguration::from_ips`], but with an additional `server_hostname` for certificate verification if it is not just the server IP.
    pub fn from_ips_with_name(
        server_ip: impl Into<IpAddr>,
        server_port: u16,
        server_hostname: String,
        local_bind_ip: impl Into<IpAddr>,
        local_bind_port: u16,
    ) -> Self {
        Self::from_addrs_with_name(
            SocketAddr::new(server_ip.into(), server_port),
            server_hostname,
            SocketAddr::new(local_bind_ip.into(), local_bind_port),
        )
    }

    /// Creates a new ClientAddrConfiguration
    ///
    /// # Arguments
    ///
    /// * `server_addr` - IP address and port of the server
    /// * `local_bind_addr` - Local address and port to bind to. For the local port to bind to, use 0 to get an OS-assigned port.
    ///
    /// # Examples
    ///
    /// Connect to an IPv4 server hosted on localhost (127.0.0.1), which is listening on port 6000. Use 0 as a local bind port to let the OS assign a port.
    /// ```
    /// use roundo_quinnet::client::connection::ClientAddrConfiguration;
    /// use std::{net::{IpAddr, Ipv4Addr, SocketAddr}};
    /// let config = ClientAddrConfiguration::from_addrs(
    ///        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 6000),
    ///        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0),
    ///    );
    /// ```
    pub fn from_addrs(server_addr: SocketAddr, local_bind_addr: SocketAddr) -> Self {
        Self::from_addrs_with_name(server_addr, server_addr.ip().to_string(), local_bind_addr)
    }

    /// Same as [`ClientAddrConfiguration::from_addrs`], but with an additional `server_hostname` for certificate verification if it is not just the server IP.
    pub fn from_addrs_with_name(
        server_addr: SocketAddr,
        server_hostname: String,
        local_bind_addr: SocketAddr,
    ) -> Self {
        Self {
            server_addr,
            server_hostname,
            local_bind_addr,
        }
    }
}

/// Current state of a client connection
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ConnectionState {
    /// The connection is currently attempting to connect to the specified server
    Connecting,
    /// The connection is currently connected to the specified server
    Connected,
    /// The connection is currently disconnected from the specified server.
    ///
    /// It may have never been connected if the connection failed.
    Disconnected,
}
impl From<&InternalConnectionState> for ConnectionState {
    fn from(internal_conn: &InternalConnectionState) -> Self {
        match internal_conn {
            InternalConnectionState::Connecting => ConnectionState::Connecting,
            InternalConnectionState::Connected(_) => ConnectionState::Connected,
            InternalConnectionState::Disconnected => ConnectionState::Disconnected,
        }
    }
}

/// Current state of a client connection
#[derive(Debug)]
pub(crate) enum InternalConnectionState {
    Connecting,
    Connected(InternalConnectionRef),
    Disconnected,
}

pub(crate) type ClientAsyncMsgSend = mpsc::Sender<ClientAsyncMessage>;
pub(crate) type ClientAsyncMsgRecv = mpsc::Receiver<ClientAsyncMessage>;

pub(crate) struct AsyncConnectionConfig {
    pub local_id: ConnectionLocalId,
    pub endpoint_config: ClientAddrConfiguration,
    pub cert_mode: CertificateVerificationMode,
}

pub(crate) fn create_client_connection_async_channels() -> (
    PayloadSend,
    PayloadRecv,
    ClientAsyncMsgSend,
    ClientAsyncMsgRecv,
    ChannelAsyncMsgSend,
    ChannelAsyncMsgRecv,
    ChannelSyncMsgSend,
    ChannelSyncMsgRecv,
    CloseSend,
    CloseRecv,
) {
    let (bytes_from_server_send, bytes_from_server_recv) =
        mpsc::channel::<(ChannelId, Bytes)>(DEFAULT_MESSAGE_QUEUE_SIZE);
    let (to_sync_client_send, to_sync_client_recv) =
        mpsc::channel::<ClientAsyncMessage>(DEFAULT_INTERNAL_MESSAGES_CHANNEL_SIZE);
    let (from_channels_send, from_channels_recv) =
        mpsc::channel::<ChannelAsyncMessage>(DEFAULT_INTERNAL_MESSAGES_CHANNEL_SIZE);
    let (to_channels_send, to_channels_recv) =
        mpsc::channel::<ChannelSyncMessage>(DEFAULT_QCHANNEL_MESSAGES_CHANNEL_SIZE);
    let (close_send, close_recv) = broadcast::channel(DEFAULT_KILL_MESSAGE_QUEUE_SIZE);
    (
        bytes_from_server_send,
        bytes_from_server_recv,
        to_sync_client_send,
        to_sync_client_recv,
        from_channels_send,
        from_channels_recv,
        to_channels_send,
        to_channels_recv,
        close_send,
        close_recv,
    )
}

/// A connection to a server from the client's perspective.
pub type ClientSideConnection = PeerConnection<ClientConnection>;

/// Specific data for a client-side connection
pub struct ClientConnection {
    /// Non-networked identifier
    local_id: ConnectionLocalId,
    /// Handle to the async runtime
    runtime: runtime::Handle,
    // Configuration
    addr_config: ClientAddrConfiguration,
    cert_mode: CertificateVerificationMode,
    channels_config: SendChannelsConfiguration,

    state: InternalConnectionState,
    send_channel_ids: ChannelsIdsPool,
    from_async_client_recv: mpsc::Receiver<ClientAsyncMessage>,
}
impl ClientConnection {
    pub(crate) fn new(
        local_id: ConnectionLocalId,
        runtime: runtime::Handle,
        client_config: ClientAddrConfiguration,
        cert_mode: CertificateVerificationMode,
        channels_config: SendChannelsConfiguration,
        from_async_client_recv: mpsc::Receiver<ClientAsyncMessage>,
    ) -> Self {
        Self {
            local_id,
            runtime,
            addr_config: client_config,
            cert_mode,
            channels_config,
            state: InternalConnectionState::Connecting,
            send_channel_ids: ChannelsIdsPool::new(),
            from_async_client_recv,
        }
    }
}
impl ClientSideConnection {
    /// Sends the payload to the server on the specified channel
    ///
    /// Will return an [`Err`] if:
    /// - the channel does not exist/is closed
    /// - or if the client is disconnected
    /// - (or if the message queue is full)
    pub fn send_payload_on<T: Into<Bytes>, C: Into<ChannelId>>(
        &mut self,
        channel_id: C,
        payload: T,
    ) -> Result<(), ClientSendError> {
        let channel_id = channel_id.into();
        match &self.specific.state {
            InternalConnectionState::Disconnected => Err(ClientSendError::ConnectionClosed),
            _ => Ok(self.internal_send_payload(channel_id, payload.into())?),
        }
    }

    /// Same as [Self::send_payload] but will log the error instead of returning it
    pub fn try_send_payload<T: Into<Bytes>>(&mut self, payload: T) {
        if let Err(err) = self.send_payload(payload) {
            error!("try_send_payload: {}", err);
        }
    }

    /// Same as [Self::send_payload_on] but will log the error instead of returning it
    pub fn try_send_payload_on<T: Into<Bytes>, C: Into<ChannelId>>(
        &mut self,
        channel_id: C,
        payload: T,
    ) {
        if let Err(err) = self.send_payload_on(channel_id, payload) {
            error!("try_send_payload_on: {}", err);
        }
    }

    /// Same as [Self::send_payload_on] but on the default channel
    pub fn send_payload<T: Into<Bytes>>(
        &mut self,
        payload: T,
    ) -> Result<(), ClientPayloadSendError> {
        match self.specific.send_channel_ids.default_channel() {
            Some(channel) => Ok(self.send_payload_on(channel, payload.into())?),
            None => Err(ClientPayloadSendError::NoDefaultChannel),
        }
    }

    /// Opens a channel of the requested [ChannelConfig] and returns its [ChannelId].
    ///
    /// If no channels were previously opened, the opened channel will be the new default channel.
    ///
    /// Can fail if the Connection is closed.
    pub fn open_channel(
        &mut self,
        channel_config: ChannelConfig,
    ) -> Result<ChannelId, ChannelCreationError> {
        let channel_id = self.specific.send_channel_ids.take_id()?;
        self.create_connection_channel(channel_id, channel_config)?;
        Ok(channel_id)
    }

    /// Closes the channel with the corresponding [ChannelId].
    ///
    /// No new messages will be able to be sent on this channel, however, the channel will properly try to send all the messages that were previously pushed to it, according to its [ChannelConfig], before fully closing.
    ///
    /// If the closed channel is the current default channel, the default channel gets set to `None`.
    ///
    /// Can fail if the [ChannelId] is unknown, or if the channel is already closed.
    pub fn close_channel(&mut self, channel_id: ChannelId) -> Result<(), ChannelCloseError> {
        self.internal_close_channel(channel_id)?;
        self.specific.send_channel_ids.release_id(channel_id);
        Ok(())
    }

    pub(crate) fn open_configured_channels(
        &mut self,
        channel_configs: SendChannelsConfiguration,
    ) -> Result<(), AsyncChannelError> {
        for channel_config in channel_configs.configs() {
            // Unchecked because we know we have enough ids
            let channel_id = self.specific.send_channel_ids.take_id().unwrap();
            match self.create_unregistered_connection_channel(channel_id, *channel_config) {
                Ok(channel) => self.register_connection_channel(channel),
                Err(e) => {
                    self.specific.send_channel_ids.release_id(channel_id);
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn internal_disconnect(
        &mut self,
        reason: CloseReason,
    ) -> Result<(), ClientConnectionCloseError> {
        match &self.specific.state {
            &InternalConnectionState::Disconnected => Ok(()),
            _ => {
                self.specific.state = InternalConnectionState::Disconnected;
                Ok(self.close(reason)?)
            }
        }
    }

    /// Attempts to reconnect to the server.
    ///
    /// This uses the initial connection configuration. Notably, channels opened by calling [`Self::open_channel`] on the connection after it was initially opened won't be automatically re-opened.
    ///
    /// Does nothing if the connection state is not [`ConnectionState::Disconnected`]
    pub fn reconnect(&mut self) -> Result<(), AsyncChannelError> {
        if let InternalConnectionState::Disconnected = &self.internal_state() {
            let (
                bytes_from_server_send,
                bytes_from_server_recv,
                to_sync_client_send,
                to_sync_client_recv,
                from_channels_send,
                from_channels_recv,
                to_channels_send,
                to_channels_recv,
                close_send,
                close_recv,
            ) = create_client_connection_async_channels();

            // Connection state reset
            self.set_state(InternalConnectionState::Connecting);
            self.specific.send_channel_ids = ChannelsIdsPool::new();
            self.specific.from_async_client_recv = to_sync_client_recv;
            self.internal_reset(
                close_send,
                to_channels_send,
                from_channels_recv,
                bytes_from_server_recv,
                self.specific.channels_config.configs().len(),
            );

            // Re-open channels
            self.open_configured_channels(self.specific.channels_config.clone())?;

            // Async connection
            let config = AsyncConnectionConfig {
                local_id: self.specific.local_id,
                endpoint_config: self.specific.addr_config.clone(),
                cert_mode: self.specific.cert_mode.clone(),
            };
            let termination = self.termination.clone();
            self.specific.runtime.spawn(async move {
                async_connection_task(
                    config,
                    to_sync_client_send,
                    bytes_from_server_send,
                    to_channels_recv,
                    from_channels_send,
                    close_recv,
                    termination,
                )
                .await
            });
        }
        Ok(())
    }

    /// Immediately prevents new messages from being sent on the connection and signal the connection to closes all its background tasks.
    ///
    /// Before trully closing, the connection will wait for all buffered messages in all its opened channels to be properly sent according to their respective channel type.
    #[inline(always)]
    pub fn disconnect(&mut self) -> Result<(), ClientConnectionCloseError> {
        self.internal_disconnect(CloseReason::LocalOrder)
    }

    /// Same as [Self::disconnect] but will log the error instead of returning it
    pub fn try_disconnect(&mut self) {
        if let Err(err) = &self.disconnect() {
            error!("Failed to properly close clonnection: {}", err);
        }
    }

    /// Logical "Disconnect", the underlying connection si already closed/lost.
    pub(crate) fn try_disconnect_closed_connection(&mut self) {
        match self.internal_disconnect(CloseReason::ConnectionLost) {
            Ok(()) => {}
            Err(ClientConnectionCloseError::ConnectionAlreadyClosed(_)) => {
                // State is disconnected; no workers remain to notify.
                trace!("Client connection workers already terminated");
            }
            Err(error) => error!("Failed to close terminated client connection: {error}"),
        }
    }

    #[inline(always)]
    pub(crate) fn try_recv_from_async(&mut self) -> Result<ClientAsyncMessage, TryRecvError> {
        self.specific.from_async_client_recv.try_recv()
    }

    #[inline(always)]
    pub(crate) fn set_state(&mut self, state: InternalConnectionState) {
        self.specific.state = state;
    }

    #[inline(always)]
    pub(crate) fn internal_state(&self) -> &InternalConnectionState {
        &self.specific.state
    }

    /// Returns the primary terminal cause, even before the close event is pumped.
    /// Reconnect replaces this cause along with the connection's async channels.
    pub fn termination_reason(&self) -> Option<TransportFailure> {
        self.termination.reason().or_else(|| {
            if let InternalConnectionState::Connected(connection) = self.internal_state() {
                connection.close_reason().map(|error| {
                    self.termination.record(TransportFailure::connection(error)).0
                })
            } else {
                None
            }
        })
    }

    /// Returns the current [ConnectionState] of the connection
    #[inline(always)]
    pub fn state(&self) -> ConnectionState {
        (&self.specific.state).into()
    }

    /// Returns statistics about the current connection if connected.
    pub fn quinn_connection_stats(&self) -> Option<ConnectionStats> {
        match &self.internal_state() {
            InternalConnectionState::Connected(connection) => Some(connection.stats()),
            _ => None,
        }
    }

    /// See [quinn::Connection::max_datagram_size]
    pub fn max_datagram_size(&self) -> Option<usize> {
        match &self.internal_state() {
            InternalConnectionState::Connected(connection) => connection.max_datagram_size(),
            _ => None,
        }
    }

    /// Set the default channel
    #[inline(always)]
    pub fn set_default_channel(&mut self, channel_id: ChannelId) {
        self.specific
            .send_channel_ids
            .set_default_channel(channel_id);
    }

    /// Get the default Channel Id
    #[inline(always)]
    pub fn default_channel(&self) -> Option<ChannelId> {
        self.specific.send_channel_ids.default_channel()
    }

    /// Returns the certificate verification configuration used by this connection
    #[inline(always)]
    pub fn certificate_verification_mode(&self) -> &CertificateVerificationMode {
        &self.specific.cert_mode
    }

    /// Returns the channels configuration used by this connection
    pub fn channels_config(&self) -> &SendChannelsConfiguration {
        &self.specific.channels_config
    }

    /// Returns the client configuration used by this connection
    pub fn client_config(&self) -> &ClientAddrConfiguration {
        &self.specific.addr_config
    }
}

#[cfg(feature = "recv_channels")]
impl ClientSideConnection {
    /// Attempts to receive a full payload sent by the server.
    ///
    /// - Returns an [`Ok`] result containg [`Some`] if there is a message from the server in the message buffer
    /// - Returns an [`Ok`] result containg [`None`] if there is no message from the server in the message buffer
    /// - Can return an [`Err`] if the connection is closed
    pub fn receive_payload<C: Into<ChannelId>>(
        &mut self,
        channel_id: C,
    ) -> Result<Option<Bytes>, crate::client::error::ConnectionClosed> {
        match &self.internal_state() {
            InternalConnectionState::Disconnected => Err(crate::client::error::ConnectionClosed),
            _ => Ok(self.internal_receive_payload(channel_id.into())),
        }
    }

    /// Same as [Self::receive_payload] but will log the error instead of returning it
    pub fn try_receive_payload<C: Into<ChannelId>>(&mut self, channel_id: C) -> Option<Bytes> {
        match self.receive_payload(channel_id) {
            Ok(payload) => payload,
            Err(err) => {
                error!("try_receive_payload: {}", err);
                None
            }
        }
    }
}

pub(crate) async fn async_connection_task(
    AsyncConnectionConfig {
        local_id,
        endpoint_config,
        cert_mode,
    }: AsyncConnectionConfig,
    to_sync_client_send: ClientAsyncMsgSend,
    bytes_from_server_send: PayloadSend,
    to_channels_recv: ChannelSyncMsgRecv,
    from_channels_send: ChannelAsyncMsgSend,
    close_recv: CloseRecv,
    termination: ConnectionTermination,
) {
    info!(
        "Connection {} trying to connect to server on: {} ...",
        local_id, endpoint_config.server_addr
    );

    let client_cfg = configure_client(cert_mode, to_sync_client_send.clone())
        .expect("Failed to configure client");

    let mut endpoint = Endpoint::client(endpoint_config.local_bind_addr)
        .expect("Failed to create client endpoint");
    endpoint.set_default_client_config(client_cfg);

    let connection = endpoint
        .connect(
            endpoint_config.server_addr,
            &endpoint_config.server_hostname,
        )
        .expect("Failed to connect: configuration error")
        .await;

    match connection {
        Err(e) => {
            termination.record(TransportFailure::quic(TransportOperation::Handshake, None, e.clone()));
            // Signal connection failure
            if to_sync_client_send
                .send(ClientAsyncMessage::ConnectionFailed(e))
                .await
                .is_err()
            {
                trace!("Client owner released before connection {local_id} failure notification");
            }
        }
        Ok(connection_handle) => {
            // Spawn a task to listen for the underlying connection being closed
            {
                let conn = connection_handle.clone();
                let to_sync_client = to_sync_client_send.clone();
                let termination = termination.clone();
                tokio::spawn(async move {
                    let error = conn.closed().await;
                    let (reason, _) = termination.record(TransportFailure::connection(error));
                    if to_sync_client
                        .send(ClientAsyncMessage::ConnectionClosed(reason))
                        .await
                        .is_err()
                    {
                        trace!("Client owner released before connection {local_id} close notification");
                    }
                })
            };

            spawn_recv_channels_tasks(
                connection_handle.clone(),
                local_id,
                close_recv.resubscribe(),
                bytes_from_server_send,
            );

            spawn_send_channels_tasks_spawner(
                connection_handle.clone(),
                close_recv,
                to_channels_recv,
                from_channels_send,
                termination,
            );

            signal_connected(connection_handle, local_id, to_sync_client_send).await;
        }
    }
}

async fn signal_connected(
    connection_handle: quinn::Connection,
    connection_id: ConnectionLocalId,
    to_sync_client_send: mpsc::Sender<ClientAsyncMessage>,
) {
    // Signal connection
    if to_sync_client_send
        .send(ClientAsyncMessage::Connected(connection_handle.clone()))
        .await
        .is_err()
    {
        trace!("Client owner released before connection {connection_id} registration");
        return;
    }

    info!(
        "Connection {} connected to {}",
        connection_id,
        connection_handle.remote_address()
    );
}

fn configure_client(
    cert_mode: CertificateVerificationMode,
    to_sync_client: mpsc::Sender<ClientAsyncMessage>,
) -> Result<ClientConfig, Box<dyn Error>> {
    let mut crypto = match cert_mode {
        CertificateVerificationMode::SkipVerification => rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth(),
        CertificateVerificationMode::SignedByCertificateAuthority => {
            // Using Quinn's helper `ClientConfig::with_platform_verifier` does not let us specify the CryptoProvider used,
            // and relies on the per-process default one (https://docs.rs/rustls/latest/rustls/crypto/struct.CryptoProvider.html#using-the-per-process-default-cryptoprovider) which may not be set.
            // As a library, we do not want to set it ourselves using `CryptoProvider::install_default`
            rustls::ClientConfig::builder_with_provider(
                rustls::crypto::ring::default_provider().into(),
            )
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            // We use `rustls-platform-verifier::with_platform_verifier` directly instead (used internally by Quinn).
            .with_platform_verifier()?
            .with_no_client_auth()
        }
        CertificateVerificationMode::TrustOnFirstUse(config) => {
            let (store, store_file) = load_known_hosts_store_from_config(config.known_hosts)?;
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(TofuServerVerification::new(
                    store,
                    config.verifier_behaviour,
                    to_sync_client,
                    store_file,
                    Arc::new(rustls::crypto::ring::default_provider()),
                ))
                .with_no_client_auth()
        }
    };

    // Quinn defaults to true
    crypto.enable_early_data = true;

    Ok(ClientConfig::new(Arc::new(QuicClientConfig::try_from(
        crypto,
    )?)))
}
