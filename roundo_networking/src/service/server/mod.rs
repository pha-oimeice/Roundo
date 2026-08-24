use super::http::{HttpState, http_router};
use super::registry::ConnectionRegistry;
use super::*;

mod connection;

use connection::*;

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
        log::info!(
            "starting network server: quic_address={}, public_address={}, certificate_directory={}",
            config.quic_address,
            config.public_address,
            config.certificate_directory.display()
        );
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

    pub fn send_to_connection<Message>(
        &self,
        connection_id: ConnectionId,
        stream: StreamId,
        message: Message,
    ) -> bool
    where
        Message: Into<ServerMessage>,
    {
        let message = message.into();
        let message_kind = server_message_kind(&message);
        if server_message_stream(&message) != Some(stream) {
            log::warn!(
                "rejected server message on wrong stream: connection_id={}, stream={stream:?}, message={message_kind}",
                connection_id.0
            );
            return false;
        }
        let queued = self
            .registry
            .send_to_connection(connection_id, stream, message);
        if !queued {
            log::warn!(
                "failed to queue server message: connection_id={}, stream={stream:?}, message={message_kind}, reason=connection_unavailable",
                connection_id.0
            );
        }
        queued
    }

    pub fn send_to_session<Message>(
        &self,
        user_session: UserSession,
        stream: StreamId,
        message: Message,
    ) where
        Message: Into<ServerMessage>,
    {
        let message = message.into();
        let message_kind = server_message_kind(&message);
        if server_message_stream(&message) != Some(stream) {
            log::warn!(
                "rejected server session message on wrong stream: stream={stream:?}, message={message_kind}"
            );
            return;
        }
        let (target_count, queued_count) =
            self.registry.send_to_session(user_session, stream, message);
        log_server_fanout(
            "session",
            stream,
            message_kind,
            target_count,
            queued_count,
            Some(user_session),
        );
    }

    pub fn send_to_all<Message>(&self, stream: StreamId, message: Message)
    where
        Message: Into<ServerMessage>,
    {
        let message = message.into();
        let message_kind = server_message_kind(&message);
        if server_message_stream(&message) != Some(stream) {
            log::warn!(
                "rejected server broadcast on wrong stream: stream={stream:?}, message={message_kind}"
            );
            return;
        }
        let (target_count, queued_count) = self.registry.send_to_all(stream, message);
        log_server_fanout(
            "all",
            stream,
            message_kind,
            target_count,
            queued_count,
            None,
        );
    }

    pub fn shutdown(&self) {
        log::info!("network server shutdown requested");
        let _ = self.shutdown.send(true);
    }
}

impl Drop for ServerNetwork {
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
    let quic_endpoint = match quic::server_endpoint(config.quic_address, Arc::clone(&tls.config)) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let _ = ready_sender.send(Err(error.clone()));
            return Err(error);
        }
    };
    let quic_address = match quic_endpoint.local_addr() {
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
        quic_address,
        public_address,
    };
    log::info!(
        "network listeners started: quic_address={}, public_address={}",
        quic_address,
        public_address
    );
    let _ = ready_sender.send(Ok(addresses));

    run_quic_listener(quic_endpoint, hooks, registry, shutdown).await;
    log::info!("QUIC listener stopped; shutting down public HTTPS listener");
    http_handle.shutdown();
    match http_task.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => log::warn!("public HTTPS listener stopped: {error}"),
        Err(error) => log::warn!("public HTTPS task stopped: {error}"),
    }
    log::info!("network server stopped");
    Ok(())
}

async fn run_quic_listener(
    endpoint: quinn::Endpoint,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            incoming = endpoint.accept() => match incoming {
                Some(incoming) => {
                    let hooks = Arc::clone(&hooks);
                    let registry = registry.clone();
                    connections.spawn(async move {
                        process_server_connection(incoming, hooks, registry).await;
                    });
                }
                None => break,
            },
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    log::warn!("QUIC connection task stopped unexpectedly: {error}");
                }
            }
        }
    }
    endpoint.close(quinn::VarInt::from_u32(0), b"server shutdown");
    endpoint.wait_idle().await;
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            log::warn!("QUIC connection task stopped unexpectedly: {error}");
        }
    }
}

fn server_message_stream(message: &ServerMessage) -> Option<StreamId> {
    match message {
        ServerMessage::Game(_) => Some(StreamId::Stream0),
        ServerMessage::Resource(_) => Some(StreamId::Stream1),
        ServerMessage::Authenticated { .. } | ServerMessage::Error { .. } => None,
    }
}

fn server_message_kind(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::Authenticated { .. } => "Authenticated",
        ServerMessage::Error { .. } => "Error",
        ServerMessage::Game(message) => message.kind(),
        ServerMessage::Resource(message) => message.kind(),
    }
}

fn log_server_fanout(
    target: &'static str,
    stream: StreamId,
    message_kind: &'static str,
    target_count: usize,
    queued_count: usize,
    user_session: Option<UserSession>,
) {
    if queued_count == target_count {
        return;
    }
    match user_session {
        Some(user_session) => log::warn!(
            "partially queued server message fanout: target={target}, stream={stream:?}, user_id={}, session_id={}, message={message_kind}, recipients={queued_count}, attempted={target_count}",
            user_session.user_id.0,
            user_session.session_id.0
        ),
        None => log::warn!(
            "partially queued server message fanout: target={target}, stream={stream:?}, message={message_kind}, recipients={queued_count}, attempted={target_count}"
        ),
    }
}
