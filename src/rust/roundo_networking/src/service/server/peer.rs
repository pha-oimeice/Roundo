//! Business admission and callbacks after opaque-byte reconstruction.
use super::*;
use roundo_quinnet::server::endpoint::Endpoint;
use roundo_contracts::{
    GAME_PROTOCOL_VERSION, ProtocolErrorCode, RESOURCE_PROTOCOL_VERSION, SessionInfo,
};
use tokio::sync::oneshot;

enum Phase {
    Join,
    Resolving,
    Ready,
    Active,
    Rejecting,
}
pub(super) struct Peer {
    phase: Phase,
    address: SocketAddr,
    identity: Option<(ConnectionId, UserSession)>,
    resolution: Option<oneshot::Receiver<Result<PublicSession, String>>>,
    senders: [mpsc::Sender<ServerMessage>; 2],
    output: BatchWriter<ServerMessage>,
    input: BatchReader<ClientMessage>,
    transport: ByteTransport,
}
impl Peer {
    pub(super) fn new(address: SocketAddr, policy: OutboundQueuePolicy) -> Self {
        let (game, game_rx) = mpsc::channel(policy.game_capacity);
        let (resource, resource_rx) = mpsc::channel(policy.resource_capacity);
        Self {
            phase: Phase::Join,
            address,
            identity: None,
            resolution: None,
            senders: [game, resource],
            output: BatchWriter::new(game_rx, resource_rx),
            input: BatchReader::default(),
            transport: ByteTransport::default(),
        }
    }
    fn reject(&mut self, code: ProtocolErrorCode) {
        self.phase = Phase::Rejecting;
        self.output.control(ServerMessage::Error { code });
    }
    pub(super) fn disconnected(&self, registry: &ConnectionRegistry, hooks: &dyn ServerHooks) {
        if let Some((id, session)) = self.identity {
            if registry.unregister(id, session) {
                hooks.on_session_disconnected(id, session, self.address);
            }
        }
    }
    pub(super) fn pump(
        &mut self,
        client_id: u64,
        endpoint: &mut Endpoint,
        runtime: &tokio::runtime::Handle,
        registry: &ConnectionRegistry,
        hooks: &Arc<dyn ServerHooks>,
    ) -> Result<bool, NetworkError> {
        if let Some(reason) = endpoint.connection(client_id)
            .and_then(|connection| connection.termination_reason())
        {
            return Err(NetworkError::from_transport(reason));
        }
        if let Some(receiver) = self.resolution.as_mut() {
            match receiver.try_recv() {
                Ok(result) => {
                    self.resolution = None;
                    match result {
                        Ok(session) => {
                            let id = registry.next_connection_id().ok_or_else(|| {
                                NetworkError::new("physical connection ID space exhausted")
                            })?;
                            self.identity = Some((id, session.user_session));
                            self.phase = Phase::Ready;
                            self.output.control(ServerMessage::SessionEstablished {
                                info: SessionInfo {
                                    user_session: session.user_session,
                                },
                            });
                        }
                        Err(error) => {
                            log::warn!("session admission failed: {error}");
                            self.reject(ProtocolErrorCode::SessionRejected);
                        }
                    }
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    return Err(NetworkError::new("session admission worker stopped"));
                }
            }
        }
        for _ in 0..2 {
            let connection = endpoint
                .connection_mut(client_id)
                .ok_or_else(|| NetworkError::transport_message("Quinnet peer disappeared"))?;
            let received = connection.dequeue_undispatched_bytes_from_peer();
            let (channel, bytes) = match received {
                Ok(payload) => payload,
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err(NetworkError::transport_end(
                        connection.termination_reason(),
                        "Quinnet peer receive worker stopped without a transport termination cause",
                    ));
                }
            };
            if let Some(batch) = self.transport.receive(channel, bytes)? {
                for message in self.input.receive(&batch)? {
                    self.message(message, runtime, registry, hooks)?;
                }
            }
        }
        if self.transport.can_prepare() {
            if let Some(batch) = self.output.prepare()? {
                self.transport.prepare(batch)?;
            }
        }
        self.transport.flush(|channel, bytes| {
            server_send_result(endpoint.send_payload_on(client_id, channel, bytes))
        })?;
        // Deliver a rejection before closing rather than racing it with a close.
        Ok(matches!(self.phase, Phase::Rejecting) && self.output.idle() && self.transport.idle())
    }
    fn message(
        &mut self,
        message: ClientMessage,
        runtime: &tokio::runtime::Handle,
        registry: &ConnectionRegistry,
        hooks: &Arc<dyn ServerHooks>,
    ) -> Result<(), NetworkError> {
        match message {
            ClientMessage::JoinPublicSession {
                resource_fingerprint,
            } if matches!(self.phase, Phase::Join) => {
                if resource_fingerprint != hooks.resource_catalog_fingerprint() {
                    self.reject(ProtocolErrorCode::IncompatibleResources);
                } else {
                    let hooks = hooks.clone();
                    let (sender, receiver) = oneshot::channel();
                    runtime.spawn(async move {
                        let result = async {
                            let session = hooks.public_session().await?;
                            if !hooks
                                .session_is_open(session.user_session.session_id)
                                .await?
                            {
                                return Err("public session is closed".into());
                            }
                            Ok(session)
                        }
                        .await;
                        if sender.send(result).is_err() {
                            log::debug!("session lookup completed after peer teardown");
                        }
                    });
                    self.resolution = Some(receiver);
                    self.phase = Phase::Resolving;
                }
            }
            ClientMessage::Ready {
                game_protocol_version,
                resource_protocol_version,
            } if matches!(self.phase, Phase::Ready) => {
                if game_protocol_version != GAME_PROTOCOL_VERSION
                    || resource_protocol_version != RESOURCE_PROTOCOL_VERSION
                {
                    self.reject(ProtocolErrorCode::UnsupportedProtocolVersion);
                } else {
                    let (id, session) = self
                        .identity
                        .expect("server issued identity before readiness");
                    registry.register(id, session, self.senders.clone());
                    self.phase = Phase::Active;
                    self.output.open();
                    hooks.on_session_connected(id, session, self.address);
                }
            }
            ClientMessage::Game(message) if matches!(self.phase, Phase::Active) => {
                let (id, session) = self.identity.expect("active identity");
                hooks.on_client_game_message(id, session, message);
            }
            ClientMessage::Resource(message) if matches!(self.phase, Phase::Active) => {
                let (id, session) = self.identity.expect("active identity");
                hooks.on_client_resource_message(id, session, message);
            }
            message => {
                return Err(NetworkError::new(format!(
                    "unexpected client business message: {}",
                    message.kind()
                )));
            }
        }
        Ok(())
    }
}
