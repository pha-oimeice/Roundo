//! Reconnecting business-session adapter hosted above Quinnet's byte transport.

use super::*;
use bevy_quinnet::client::connection::ConnectionFailedEvent;
use roundo_contracts::{GAME_PROTOCOL_VERSION, RESOURCE_PROTOCOL_VERSION};

/// Nonblocking admission to independent business queues. Success means local
/// admission only. The transport has its own byte window, unrelated to these
/// message-count capacities. Shutdown joins all callbacks before returning.
pub struct ClientNetwork {
    outbound: [mpsc::Sender<ClientMessage>; 2],
    worker: Worker,
}
impl ClientNetwork {
    pub fn start(
        config: ClientNetworkConfig,
        hooks: Arc<dyn ClientHooks>,
    ) -> Result<Self, NetworkError> {
        config.outbound.validate()?;
        let (game, game_rx) = mpsc::channel(config.outbound.game_capacity);
        let (resource, resource_rx) = mpsc::channel(config.outbound.resource_capacity);
        let (worker, ()) = Worker::start(move |stop, ready| {
            run_client(
                config,
                hooks,
                BatchWriter::new(game_rx, resource_rx),
                stop,
                ready,
            )
        })?;
        Ok(Self {
            outbound: [game, resource],
            worker,
        })
    }
    pub fn send<M: Into<ClientMessage>>(&self, message: M) -> Result<(), NetworkError> {
        let message = message.into();
        let class = message
            .traffic_class()
            .ok_or_else(|| NetworkError::new("negotiation cannot enter a business queue"))?;
        match admit(&self.outbound[class as usize], message) {
            AdmissionResult::Queued => Ok(()),
            AdmissionResult::Saturated => {
                Err(NetworkError::new("business outbound queue is saturated"))
            }
            AdmissionResult::Closed => Err(NetworkError::new("network client stopped")),
        }
    }
    pub fn shutdown(&self) {
        self.worker.shutdown();
    }
}

fn run_client(
    config: ClientNetworkConfig,
    hooks: Arc<dyn ClientHooks>,
    mut output: BatchWriter<ClientMessage>,
    stop: Arc<AtomicBool>,
    ready: SyncSender<Result<(), NetworkError>>,
) -> Result<(), NetworkError> {
    let mut app = App::new();
    app.add_plugins(QuinnetClientPlugin::default());
    ready.send(Ok(())).map_err(NetworkError::from_display)?;
    let mut trusted = CertStore::new();
    let mut id = None;
    let mut retry_at = Instant::now();
    let mut connected = false;
    let mut session_ready = false;
    let mut transport = ByteTransport::default();
    let mut input = BatchReader::<ServerMessage>::default();
    run_polling_bridge(&stop, || {
        app.update();
        update_trust(&mut app, &mut trusted);
        let failure = app
            .world_mut()
            .resource_mut::<Messages<ConnectionFailedEvent>>()
            .drain()
            .find(|event| Some(event.id) == id)
            .map(|event| NetworkError::from_display(event.err));
        let step = (|| -> Result<(), NetworkError> {
            if id.is_none() {
                if Instant::now() < retry_at {
                    return Ok(());
                }
                output.reset();
                transport = ByteTransport::default();
                input = BatchReader::default();
                let configuration = client_configuration(
                    config.quic_address,
                    &config.server_name,
                    &config.certificate_policy,
                    &trusted,
                );
                id = Some(
                    app.world_mut()
                        .resource_mut::<QuinnetClient>()
                        .open_connection(configuration)
                        .map_err(NetworkError::from_display)?,
                );
            }
            if let Some(error) = failure {
                return Err(error);
            }
            let current = id.expect("opened connection");
            let mut client = app.world_mut().resource_mut::<QuinnetClient>();
            let connection = client
                .get_connection_mut_by_id(current)
                .ok_or_else(|| NetworkError::new("Quinnet connection disappeared"))?;
            match connection.state() {
                ConnectionState::Connecting => return Ok(()),
                ConnectionState::Disconnected => {
                    return Err(NetworkError::new("Quinnet connection closed"));
                }
                ConnectionState::Connected => {}
            }
            if !connected {
                output.control(ClientMessage::JoinPublicSession {
                    resource_fingerprint: hooks.resource_catalog_fingerprint(),
                });
                connected = true;
            }
            // A conforming peer has at most one data block and one ACK in flight.
            // Bound each pump's work even if an untrusted peer sends continuously.
            for _ in 0..2 {
                let (channel, bytes) = match connection.dequeue_undispatched_bytes_from_peer() {
                    Ok(payload) => payload,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        return Err(NetworkError::new("Quinnet receive worker stopped"));
                    }
                };
                if let Some(batch) = transport.receive(channel, bytes)? {
                    for message in input.receive(&batch)? {
                        match message {
                            ServerMessage::SessionEstablished { .. } if !session_ready => {
                                // No queued work from dialing/previous epochs survives
                                // the authoritative session boundary.
                                output.reset();
                                output.control(ClientMessage::Ready {
                                    game_protocol_version: GAME_PROTOCOL_VERSION,
                                    resource_protocol_version: RESOURCE_PROTOCOL_VERSION,
                                });
                                output.open();
                                session_ready = true;
                                hooks.on_connection_established();
                            }
                            ServerMessage::Game(message) if session_ready => {
                                hooks.on_server_game_message(message)
                            }
                            ServerMessage::Resource(message) if session_ready => {
                                hooks.on_server_resource_message(message)
                            }
                            ServerMessage::Error { code } => {
                                return Err(NetworkError::new(format!(
                                    "session rejected: {code:?}"
                                )));
                            }
                            message => {
                                return Err(NetworkError::new(format!(
                                    "unexpected server business message: {}",
                                    message.kind()
                                )));
                            }
                        }
                    }
                }
            }
            if transport.can_prepare() {
                if let Some(batch) = output.prepare()? {
                    transport.prepare(batch)?;
                }
            }
            transport.flush(|channel, bytes| {
                client_send_result(connection.send_payload_on(channel, bytes))
            })
        })();
        if let Err(error) = step {
            log::warn!("Quinnet client connection ended: {error}");
            if let Some(current) = id.take() {
                if let Err(close) = app
                    .world_mut()
                    .resource_mut::<QuinnetClient>()
                    .close_connection(current)
                {
                    log::debug!("Quinnet client was already closed: {close}");
                }
            }
            if session_ready {
                hooks.on_connection_lost();
            } else {
                hooks.on_connection_error(&error);
            }
            connected = false;
            session_ready = false;
            output.reset();
            input = BatchReader::default();
            transport = ByteTransport::default();
            retry_at = Instant::now() + config.reconnect_delay;
        }
        BridgeStep::Idle
    });
    app.world_mut()
        .resource_mut::<QuinnetClient>()
        .close_all_connections();
    Ok(())
}
