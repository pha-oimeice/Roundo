//! Threaded QUIC listener and non-blocking outbound routing API.

use super::registry::ConnectionRegistry;
use super::*;

mod connection;

use connection::*;

/// Running network server backed by a dedicated Tokio runtime thread.
///
/// Connection tasks register bounded per-stream senders for direct and fan-out
/// routing. Dropping the final handle requests shutdown and joins the runtime.
pub struct ServerNetwork {
    registry: ConnectionRegistry,
    shutdown: watch::Sender<bool>,
    addresses: ServerAddresses,
    worker: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}

impl ServerNetwork {
    /// Initializes host state, TLS, and the QUIC listener on a worker thread.
    ///
    /// This call blocks for startup readiness for at most ten seconds. Success
    /// means the listener is bound and accepting connections. Host initialization
    /// and TLS preparation may perform blocking database or filesystem work on
    /// the runtime worker before readiness is reported.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime creation, hook initialization, TLS setup,
    /// endpoint binding/address lookup, or bounded startup notification fails.
    pub fn start(
        config: ServerNetworkConfig,
        hooks: Arc<dyn ServerHooks>,
    ) -> Result<Self, NetworkError> {
        log::info!(
            "starting network server: quic_address={}, certificate_directory={}",
            config.quic_address,
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
                join_startup_worker(join_handle, "startup failed");
                return Err(error);
            }
            Err(RecvTimeoutError::Timeout) => {
                let shutdown_result = shutdown.send(true);
                if shutdown_result.is_err() {
                    log::debug!(
                        "network server startup timed out after worker had already stopped"
                    );
                }
                join_startup_worker(join_handle, "startup timed out");
                return Err(NetworkError::new("timed out while starting network server"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                join_startup_worker(join_handle, "startup channel disconnected");
                return Err(NetworkError::new("network server stopped before startup"));
            }
        };

        Ok(Self {
            registry,
            shutdown,
            addresses,
            worker: std::sync::Mutex::new(Some(join_handle)),
        })
    }

    /// Returns the bound listener addresses captured at successful startup.
    pub const fn addresses(&self) -> ServerAddresses {
        self.addresses
    }

    /// Attempts to enqueue one message for one connection without blocking.
    ///
    /// Returns `true` only for local queue admission. It returns `false` for a
    /// wrong logical stream, unknown/closed connection, or saturated queue; the
    /// converted message is then dropped. Admission does not imply peer delivery.
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
        let admission = self
            .registry
            .send_to_connection(connection_id, stream, message);
        match admission {
            AdmissionResult::Queued => true,
            AdmissionResult::Saturated => {
                log::warn!(
                    "rejected server message: connection_id={}, stream={stream:?}, message={message_kind}, reason=outbound_saturated",
                    connection_id.0
                );
                false
            }
            AdmissionResult::Closed => {
                log::warn!(
                    "failed to queue server message: connection_id={}, stream={stream:?}, message={message_kind}, reason=connection_unavailable",
                    connection_id.0
                );
                false
            }
        }
    }

    /// Attempts non-blocking fan-out to every connection in one user session.
    ///
    /// Each recipient is admitted independently in unspecified order. Wrong-
    /// stream messages are rejected entirely; saturation or closure may produce
    /// partial delivery, which is logged and not retried.
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

    /// Attempts non-blocking fan-out to every currently registered connection.
    ///
    /// Recipient order is unspecified. Wrong-stream messages are rejected
    /// entirely; per-recipient saturation or closure can produce partial
    /// delivery, which is logged and not retried.
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

    /// Stops the network worker and waits for its listener and connection tasks.
    ///
    /// This blocks the current OS thread. It is idempotent, closes the QUIC
    /// endpoint, and waits for connection tasks; worker panics are logged rather
    /// than propagated. Dropping the handle performs the same shutdown.
    pub fn shutdown(&self) {
        log::info!("network server shutdown requested");
        let shutdown_result = self.shutdown.send(true);
        if shutdown_result.is_err() {
            log::debug!("network server shutdown receiver was already closed");
        }
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.thread().id() == thread::current().id() {
                log::error!("network server worker attempted to join itself during shutdown");
                return;
            }
            if let Err(error) = worker.join() {
                log::error!("network server worker panicked during shutdown: {error:?}");
            }
        }
    }
}

impl Drop for ServerNetwork {
    fn drop(&mut self) {
        self.shutdown();
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
        notify_startup(&ready_sender, Err(error.clone()), "hook initialization");
        return Err(error);
    }

    let tls = match tls::prepare_server_tls(
        &config.certificate_directory,
        &config.server_alternative_names,
        config.generate_self_signed_certificate,
    ) {
        Ok(tls) => tls,
        Err(error) => {
            notify_startup(&ready_sender, Err(error.clone()), "TLS preparation");
            return Err(error);
        }
    };
    let quic_endpoint = match quic::server_endpoint(config.quic_address, Arc::clone(&tls.config)) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            notify_startup(&ready_sender, Err(error.clone()), "QUIC endpoint creation");
            return Err(error);
        }
    };
    let quic_address = match quic_endpoint.local_addr() {
        Ok(address) => address,
        Err(error) => {
            let error = NetworkError::from_display(error);
            notify_startup(&ready_sender, Err(error.clone()), "listener address lookup");
            return Err(error);
        }
    };
    let addresses = ServerAddresses { quic_address };
    log::info!("network listener started: quic_address={quic_address}");
    notify_startup(&ready_sender, Ok(addresses), "listener readiness");

    run_quic_listener(quic_endpoint, hooks, registry, config.admission, shutdown).await;
    log::info!("network server stopped");
    Ok(())
}

fn join_startup_worker(worker: thread::JoinHandle<()>, context: &str) {
    if let Err(error) = worker.join() {
        log::error!("network server worker panicked while handling {context}: {error:?}");
    }
}

fn notify_startup(
    sender: &SyncSender<Result<ServerAddresses, NetworkError>>,
    result: Result<ServerAddresses, NetworkError>,
    stage: &str,
) {
    let notification = sender.send(result);
    if notification.is_err() {
        log::warn!(
            "cannot report network server startup result: stage={stage}, reason=startup_receiver_closed"
        );
    }
}

// Accepts connections concurrently; shutdown closes the endpoint and joins every task.
async fn run_quic_listener(
    endpoint: quinn::Endpoint,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
    admission: TransportAdmissionPolicy,
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
                        process_server_connection(incoming, hooks, registry, admission).await;
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
    let () = endpoint.close(quinn::VarInt::from_u32(0), b"server shutdown");
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
        ServerMessage::SessionEstablished { .. } | ServerMessage::Error { .. } => None,
    }
}

fn server_message_kind(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::SessionEstablished { .. } => "SessionEstablished",
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
