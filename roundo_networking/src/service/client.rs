//! Reconnecting QUIC client hosted on a dedicated Tokio runtime thread.

use super::*;

/// Handle for one reconnecting network client and its bounded outbound queues.
///
/// The worker reconnects until [`shutdown`](Self::shutdown) or drop. Application
/// callbacks run on the networking runtime, not on the thread that owns this
/// handle. The handle itself does not expose connection state.
pub struct ClientNetwork {
    stream0_outbound: mpsc::Sender<ClientMessage>,
    stream1_outbound: mpsc::Sender<ClientMessage>,
    shutdown: watch::Sender<bool>,
    worker: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}

impl ClientNetwork {
    /// Creates the Tokio runtime and starts a reconnecting worker thread.
    ///
    /// Success means only that the local runtime and queues were created. TLS
    /// setup, dialing, and session negotiation happen asynchronously. Dial and
    /// negotiation failures reach [`ClientHooks::on_connection_error`]; TLS
    /// setup failure is logged and stops the worker. Configured queue capacities
    /// are clamped to at least one item.
    ///
    /// # Errors
    ///
    /// Returns an error if the Tokio runtime cannot be created.
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
        let admission = config.admission;
        let (stream0_outbound, stream0_receiver) =
            mpsc::channel(admission.capacity(StreamId::Stream0).max(1));
        let (stream1_outbound, stream1_receiver) =
            mpsc::channel(admission.capacity(StreamId::Stream1).max(1));
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

    /// Attempts to enqueue one application message without blocking.
    ///
    /// Admission success guarantees only insertion into the selected local
    /// queue; it does not imply an active connection or peer delivery. Messages
    /// not consumed by the current transport attempt are discarded at the next
    /// session boundary and never migrate to another authoritative session.
    /// Session-negotiation messages cannot be submitted through this method.
    ///
    /// # Errors
    ///
    /// Returns an error when the message belongs to another stream, the bounded
    /// queue is full, or the worker has stopped. Messages are scoped to the
    /// currently establishing/active transport attempt: anything still queued
    /// when a later session is established is discarded before that session's
    /// callback. The converted message is not returned on failure.
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
        let admission = match stream {
            StreamId::Stream0 => admit(&self.stream0_outbound, message),
            StreamId::Stream1 => admit(&self.stream1_outbound, message),
        };
        match admission {
            AdmissionResult::Queued => Ok(()),
            AdmissionResult::Saturated => {
                log::warn!(
                    "rejected client message: stream={stream:?}, message={message_kind}, reason=outbound_saturated"
                );
                Err(NetworkError::new("network outbound queue is saturated"))
            }
            AdmissionResult::Closed => {
                log::warn!(
                    "failed to queue client message: stream={stream:?}, message={message_kind}, reason=network_client_stopped"
                );
                Err(NetworkError::new("network client is no longer running"))
            }
        }
    }

    /// Requests shutdown and waits for reconnect and stream I/O to end.
    ///
    /// This blocks the current OS thread while joining the worker. Calling it
    /// more than once is harmless; dropping the handle performs the same step.
    /// A worker panic is logged rather than propagated.
    pub fn shutdown(&self) {
        log::info!("network client shutdown requested");
        let shutdown_result = self.shutdown.send(true);
        if shutdown_result.is_err() {
            log::debug!("network client shutdown receiver was already closed");
        }
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
    mut stream0_outbound: mpsc::Receiver<ClientMessage>,
    mut stream1_outbound: mpsc::Receiver<ClientMessage>,
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
            result = establish_client_sessions(
                &config,
                Arc::clone(&tls_config),
                hooks.resource_catalog_fingerprint(),
            ) => result,
        };
        match establishment {
            Ok(sessions) => {
                log::info!(
                    "QUIC stream0 and stream1 established: quic_address={}, server_name={}",
                    config.quic_address,
                    config.server_name
                );
                let (discarded_stream0, discarded_stream1) =
                    discard_stale_outbound(&mut stream0_outbound, &mut stream1_outbound);
                if discarded_stream0 != 0 || discarded_stream1 != 0 {
                    log::warn!(
                        "discarded stale client outbound messages at session boundary: stream0={discarded_stream0}, stream1={discarded_stream1}"
                    );
                }
                hooks.on_connection_established();
                run_client_sessions(
                    sessions,
                    Arc::clone(&hooks),
                    config.admission,
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

fn discard_stale_outbound(
    stream0: &mut mpsc::Receiver<ClientMessage>,
    stream1: &mut mpsc::Receiver<ClientMessage>,
) -> (usize, usize) {
    fn drain(receiver: &mut mpsc::Receiver<ClientMessage>) -> usize {
        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }
        count
    }
    (drain(stream0), drain(stream1))
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
    resource_fingerprint: ResourceCatalogFingerprint,
) -> Result<ClientSessions, NetworkError> {
    let endpoint = quic::client_endpoint(config.quic_address, tls_config)?;
    let connection = quic::dial(&endpoint, config.quic_address, &config.server_name).await?;
    let streams = quic::establish_streams(&connection).await?;
    let established = ClientSession::new(streams.stream0)
        .join_public_session(resource_fingerprint)
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
    let stream1 = ClientResourceSession::initialize(streams.stream1)
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
    admission: TransportAdmissionPolicy,
    stream0_outbound: &mut mpsc::Receiver<ClientMessage>,
    stream1_outbound: &mut mpsc::Receiver<ClientMessage>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let ClientSessions {
        endpoint,
        connection,
        stream0,
        stream1,
    } = sessions;
    let mut stream0 = ConnectionIo::spawn(
        stream0.into_connection(),
        admission.capacity(StreamId::Stream0),
    );
    let mut stream1 = ConnectionIo::spawn(
        stream1.into_connection(),
        admission.capacity(StreamId::Stream1),
    );
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
                    if stream0.admit_message(message) != AdmissionResult::Queued {
                        log::warn!("failed to forward client stream0 message: message={message_kind}, reason=connection_writer_stopped");
                        break;
                    }
                }
                None => break,
            },
            message = stream1_outbound.recv() => match message {
                Some(message) => {
                    let message_kind = client_message_kind(&message);
                    if stream1.admit_message(message) != AdmissionResult::Queued {
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
    let () = connection.close(quinn::VarInt::from_u32(0), b"client session closed");
    drop(stream0);
    drop(stream1);
    endpoint.wait_idle().await;
}

fn client_message_stream(message: &ClientMessage) -> Option<StreamId> {
    match message {
        ClientMessage::Game(_) => Some(StreamId::Stream0),
        ClientMessage::Resource(_) => Some(StreamId::Stream1),
        ClientMessage::JoinPublicSession { .. } | ClientMessage::Ready { .. } => None,
    }
}

fn client_message_kind(message: &ClientMessage) -> &'static str {
    match message {
        ClientMessage::JoinPublicSession { .. } => "JoinPublicSession",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_transport_session_discards_all_queued_application_messages() {
        let (stream0_sender, mut stream0) = mpsc::channel(4);
        let (stream1_sender, mut stream1) = mpsc::channel(4);
        stream0_sender
            .try_send(ClientMessage::Ready {
                stream: StreamId::Stream0,
                protocol_version: 1,
            })
            .unwrap();
        stream1_sender
            .try_send(ClientMessage::Ready {
                stream: StreamId::Stream1,
                protocol_version: 1,
            })
            .unwrap();

        assert_eq!(discard_stale_outbound(&mut stream0, &mut stream1), (1, 1));
        assert!(stream0.try_recv().is_err());
        assert!(stream1.try_recv().is_err());
    }
}
