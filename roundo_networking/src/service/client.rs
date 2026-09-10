use super::*;

pub struct ClientNetwork {
    stream0_outbound: mpsc::UnboundedSender<ClientMessage>,
    stream1_outbound: mpsc::UnboundedSender<ClientMessage>,
    shutdown: watch::Sender<bool>,
    worker: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}

impl ClientNetwork {
    pub fn start(
        config: ClientNetworkConfig,
        hooks: Arc<dyn ClientHooks>,
    ) -> Result<Self, NetworkError> {
        log::info!(
            "starting network client: quic_address={}, server_name={}, certificate_policy={:?}, reconnect_delay_ms={}",
            config.quic_address,
            config.server_name,
            config.certificate_policy,
            config.reconnect_delay.as_millis()
        );
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(NetworkError::from_display)?;
        let (stream0_outbound, stream0_receiver) = mpsc::unbounded_channel();
        let (stream1_outbound, stream1_receiver) = mpsc::unbounded_channel();
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let worker = thread::spawn(move || {
            runtime.block_on(run_client(
                config,
                hooks,
                stream0_receiver,
                stream1_receiver,
                shutdown_receiver,
            ));
        });

        Ok(Self {
            stream0_outbound,
            stream1_outbound,
            shutdown,
            worker: std::sync::Mutex::new(Some(worker)),
        })
    }

    pub fn send<Message>(&self, stream: StreamId, message: Message) -> Result<(), NetworkError>
    where
        Message: Into<ClientMessage>,
    {
        let message = message.into();
        let message_kind = client_message_kind(&message);
        if client_message_stream(&message) != Some(stream) {
            return Err(NetworkError::new(format!(
                "client message {message_kind} cannot be sent on {stream:?}"
            )));
        }
        let result = match stream {
            StreamId::Stream0 => self.stream0_outbound.send(message),
            StreamId::Stream1 => self.stream1_outbound.send(message),
        };
        match result {
            Ok(()) => Ok(()),
            Err(_) => {
                log::warn!(
                    "failed to queue client message: stream={stream:?}, message={message_kind}, reason=network_client_stopped"
                );
                Err(NetworkError::new("network client is no longer running"))
            }
        }
    }

    /// Stops the network runtime and waits until reconnect and stream I/O have ended.
    /// Calling it more than once is harmless.
    pub fn shutdown(&self) {
        log::info!("network client shutdown requested");
        let _ = self.shutdown.send(true);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.thread().id() == thread::current().id() {
                log::error!("network client worker attempted to join itself during shutdown");
                return;
            }
            if let Err(error) = worker.join() {
                log::error!("network client worker panicked during shutdown: {error:?}");
            }
        }
    }
}

impl Drop for ClientNetwork {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn run_client(
    config: ClientNetworkConfig,
    hooks: Arc<dyn ClientHooks>,
    mut stream0_outbound: mpsc::UnboundedReceiver<ClientMessage>,
    mut stream1_outbound: mpsc::UnboundedReceiver<ClientMessage>,
    mut shutdown: watch::Receiver<bool>,
) {
    let tls_config = match tls::create_client_tls_config(&config.certificate_policy) {
        Ok(config) => config,
        Err(error) => {
            log::error!("failed to configure QUIC TLS client: {error}");
            return;
        }
    };
    let mut attempt = 0_u64;
    loop {
        if *shutdown.borrow() {
            log::info!("network client stopped before connection attempt");
            return;
        }
        attempt += 1;
        log::info!(
            "connecting to QUIC server: attempt={attempt}, quic_address={}",
            config.quic_address
        );
        let establishment = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    log::info!("network client stopped during connection establishment");
                    return;
                }
                continue;
            }
            result = establish_client_sessions(&config, Arc::clone(&tls_config)) => result,
        };
        match establishment {
            Ok(sessions) => {
                log::info!(
                    "QUIC stream0 and stream1 established: quic_address={}, server_name={}",
                    config.quic_address,
                    config.server_name
                );
                hooks.on_connection_established();
                run_client_sessions(
                    sessions,
                    Arc::clone(&hooks),
                    &mut stream0_outbound,
                    &mut stream1_outbound,
                    &mut shutdown,
                )
                .await;
                if !*shutdown.borrow() {
                    hooks.on_connection_lost();
                }
            }
            Err(error) => {
                log::warn!("network connection failed: {error}");
                hooks.on_connection_error(&error);
            }
        }
        if *shutdown.borrow() {
            log::info!("network client stopped");
            return;
        }
        log::info!(
            "scheduling network reconnect: delay_ms={}",
            config.reconnect_delay.as_millis()
        );
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    log::info!("network client stopped during reconnect delay");
                    return;
                }
            }
            _ = tokio::time::sleep(config.reconnect_delay) => {}
        }
    }
}

struct ClientSessions {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    stream0: ClientGameSession<quic::PairedStream>,
    stream1: ClientResourceSession<quic::PairedStream>,
}

async fn establish_client_sessions(
    config: &ClientNetworkConfig,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<ClientSessions, NetworkError> {
    let endpoint = quic::client_endpoint(config.quic_address, tls_config)?;
    let connection = quic::connect(&endpoint, config.quic_address, &config.server_name).await?;
    let streams = quic::establish_streams(&connection).await?;
    let established = ClientSession::new(streams.stream0)
        .join_public_session()
        .await
        .map_err(NetworkError::from_display)?;
    let session_info = established.session_info();
    log::debug!(
        "QUIC public session established: user_id={}, session_id={}",
        session_info.user_session.user_id.0,
        session_info.user_session.session_id.0
    );
    let stream0 = established
        .enter_game()
        .await
        .map_err(NetworkError::from_display)?;
    let stream1 = ClientResourceSession::open(streams.stream1)
        .await
        .map_err(NetworkError::from_display)?;
    Ok(ClientSessions {
        endpoint,
        connection,
        stream0,
        stream1,
    })
}

async fn run_client_sessions(
    sessions: ClientSessions,
    hooks: Arc<dyn ClientHooks>,
    stream0_outbound: &mut mpsc::UnboundedReceiver<ClientMessage>,
    stream1_outbound: &mut mpsc::UnboundedReceiver<ClientMessage>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let ClientSessions {
        endpoint,
        connection,
        stream0,
        stream1,
    } = sessions;
    let mut stream0 = ConnectionIo::spawn(stream0.into_connection());
    let mut stream1 = ConnectionIo::spawn(stream1.into_connection());
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    log::debug!("closing active QUIC connection after shutdown request");
                    break;
                }
            }
            message = stream0_outbound.recv() => match message {
                Some(message) => {
                    let message_kind = client_message_kind(&message);
                    if !stream0.send(message) {
                        log::warn!("failed to forward client stream0 message: message={message_kind}, reason=connection_writer_stopped");
                        break;
                    }
                }
                None => break,
            },
            message = stream1_outbound.recv() => match message {
                Some(message) => {
                    let message_kind = client_message_kind(&message);
                    if !stream1.send(message) {
                        log::warn!("failed to forward client stream1 message: message={message_kind}, reason=connection_writer_stopped");
                        break;
                    }
                }
                None => break,
            },
            event = stream0.receive() => match event {
                Some(ConnectionIoEvent::Message(ServerMessage::Game(message))) => {
                    hooks.on_server_game_message(message);
                }
                Some(ConnectionIoEvent::Message(message)) => {
                    log::warn!("unexpected server stream0 message: message={}", server_message_kind(&message));
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) if error.is_peer_disconnect() => {
                    log::info!("QUIC server disconnected: stream=stream0, side={}", side.name());
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) => {
                    log::warn!("stream0 I/O failed: side={}, error={error}", side.name());
                    break;
                }
                Some(ConnectionIoEvent::Stopped { .. }) | None => break,
            },
            event = stream1.receive() => match event {
                Some(ConnectionIoEvent::Message(ServerMessage::Resource(message))) => {
                    hooks.on_server_resource_message(message);
                }
                Some(ConnectionIoEvent::Message(message)) => {
                    log::warn!("unexpected server stream1 message: message={}", server_message_kind(&message));
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) if error.is_peer_disconnect() => {
                    log::info!("QUIC server disconnected: stream=stream1, side={}", side.name());
                    break;
                }
                Some(ConnectionIoEvent::Stopped { side, result: Err(error) }) => {
                    log::warn!("stream1 I/O failed: side={}, error={error}", side.name());
                    break;
                }
                Some(ConnectionIoEvent::Stopped { .. }) | None => break,
            },
        }
    }
    stream0.shutdown();
    stream1.shutdown();
    connection.close(quinn::VarInt::from_u32(0), b"client session closed");
    drop(stream0);
    drop(stream1);
    endpoint.wait_idle().await;
}

fn client_message_stream(message: &ClientMessage) -> Option<StreamId> {
    match message {
        ClientMessage::Game(_) => Some(StreamId::Stream0),
        ClientMessage::Resource(_) => Some(StreamId::Stream1),
        ClientMessage::JoinPublicSession | ClientMessage::Ready { .. } => None,
    }
}

fn client_message_kind(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::JoinPublicSession => "JoinPublicSession",
        ClientMessage::Ready { .. } => "Ready",
        ClientMessage::Game(message) => message.kind(),
        ClientMessage::Resource(message) => message.kind(),
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
