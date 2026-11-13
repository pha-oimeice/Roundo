//! Quinnet listener and business-session routing facade.
use super::registry::ConnectionRegistry;
use super::*;
use roundo_quinnet::{
    server::{
        EndpointAddrConfiguration, QuinnetServer, QuinnetServerPlugin, ServerEndpointConfiguration,
        ServerEndpointConfigurationDefaultables,
    },
    shared::AsyncRuntime,
};
mod peer;
use peer::Peer;

#[derive(Debug)]
pub enum ServerSendError {
    Saturated(ServerMessage),
    Closed,
    NegotiationMessage,
}
pub struct ServerNetwork {
    registry: ConnectionRegistry,
    worker: Worker,
    addresses: ServerAddresses,
}
impl ServerNetwork {
    pub fn start(
        config: ServerNetworkConfig,
        hooks: Arc<dyn ServerHooks>,
    ) -> Result<Self, NetworkError> {
        config.outbound.validate()?;
        let registry = ConnectionRegistry::default();
        let worker_registry = registry.clone();
        let (worker, addresses) = Worker::start(move |stop, ready| {
            run_server(config, hooks, worker_registry, stop, ready)
        })?;
        Ok(Self {
            registry,
            worker,
            addresses,
        })
    }
    pub const fn addresses(&self) -> ServerAddresses {
        self.addresses
    }
    /// Success means business-queue admission, not byte submission or delivery.
    pub fn send_to_connection<M: Into<ServerMessage>>(&self, id: ConnectionId, message: M) -> bool {
        match self.try_send_to_connection(id, message) {
            Ok(()) => true,
            Err(error) => {
                let reason = match error {
                    ServerSendError::Saturated(_) => "business_queue_saturated",
                    ServerSendError::Closed => "connection_closed",
                    ServerSendError::NegotiationMessage => "negotiation_in_business_queue",
                };
                log::warn!(
                    "business send rejected: connection={}, reason={reason}",
                    id.0
                );
                false
            }
        }
    }
    pub fn try_send_to_connection<M: Into<ServerMessage>>(
        &self,
        id: ConnectionId,
        message: M,
    ) -> Result<(), ServerSendError> {
        let message = message.into();
        let class = message
            .traffic_class()
            .ok_or(ServerSendError::NegotiationMessage)?;
        self.registry
            .try_send_to_connection(id, class, message)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(message) => ServerSendError::Saturated(message),
                mpsc::error::TrySendError::Closed(_) => ServerSendError::Closed,
            })
    }
    pub fn send_to_session<M: Into<ServerMessage>>(&self, session: UserSession, message: M) {
        let message = message.into();
        let Some(class) = message.traffic_class() else {
            log::warn!("negotiation message rejected from session fanout");
            return;
        };
        let (targets, queued) = self.registry.send_to_session(session, class, message);
        if targets != queued {
            log::warn!("partial session business fanout: {queued}/{targets}");
        }
    }
    pub fn send_to_all<M: Into<ServerMessage>>(&self, message: M) {
        let message = message.into();
        let Some(class) = message.traffic_class() else {
            log::warn!("negotiation message rejected from broadcast");
            return;
        };
        let (targets, queued) = self.registry.send_to_all(class, message);
        if targets != queued {
            log::warn!("partial business broadcast: {queued}/{targets}");
        }
    }
    pub fn shutdown(&self) {
        self.worker.shutdown();
    }
}

fn run_server(
    config: ServerNetworkConfig,
    hooks: Arc<dyn ServerHooks>,
    registry: ConnectionRegistry,
    stop: Arc<AtomicBool>,
    ready: SyncSender<Result<ServerAddresses, NetworkError>>,
) -> Result<(), NetworkError> {
    let mut app = App::new();
    app.add_plugins(QuinnetServerPlugin::default());
    let runtime = app.world().resource::<AsyncRuntime>().handle().clone();
    let (initialized, mut initialization) = tokio::sync::oneshot::channel();
    let initialize_hooks = hooks.clone();
    runtime.spawn(async move {
        let _ = initialized.send(initialize_hooks.initialize().await);
    });
    let mut initialized = None;
    run_polling_bridge(&stop, || match initialization.try_recv() {
        Ok(result) => {
            initialized = Some(result.map_err(NetworkError::new));
            BridgeStep::Stop
        }
        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => BridgeStep::Idle,
        Err(error) => {
            initialized = Some(Err(NetworkError::from_display(error)));
            BridgeStep::Stop
        }
    });
    initialized.ok_or_else(|| NetworkError::new("server startup cancelled"))??;
    let certificate = tls::server_files(&config)?;
    let addresses = {
        let mut server = app.world_mut().resource_mut::<QuinnetServer>();
        server
            .start_endpoint(ServerEndpointConfiguration {
                addr_config: EndpointAddrConfiguration::from_addr(config.quic_address),
                cert_mode: certificate,
                defaultables: ServerEndpointConfigurationDefaultables {
                    send_channels_cfg: transport::channels(),
                },
            })
            .map_err(NetworkError::from_display)?;
        ServerAddresses {
            quic_address: server.endpoint().local_addr(),
        }
    };
    ready
        .send(Ok(addresses))
        .map_err(NetworkError::from_display)?;
    let mut peers = HashMap::<u64, Peer>::new();
    run_polling_bridge(&stop, || {
        app.update();
        for event in app.world_mut()
            .resource_mut::<Messages<roundo_quinnet::server::ConnectionLostEvent>>()
            .drain()
        {
            log_transport_end(
                &format!("client_id={}", event.id),
                &NetworkError::from_transport(event.reason),
            );
        }
        let mut server = app.world_mut().resource_mut::<QuinnetServer>();
        let endpoint = server.endpoint_mut();
        let current: HashSet<_> = endpoint.clients().into_iter().collect();
        let gone: Vec<_> = peers
            .keys()
            .filter(|id| !current.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(peer) = peers.remove(&id) {
                peer.disconnected(&registry, hooks.as_ref());
            }
        }
        for id in current {
            let peer = peers.entry(id).or_insert_with(|| {
                Peer::new(
                    endpoint
                        .connection(id)
                        .expect("published Quinnet peer")
                        .remote_addr(),
                    config.outbound,
                )
            });
            let close = match peer.pump(id, endpoint, &runtime, &registry, &hooks) {
                Ok(close) => close,
                Err(error) => {
                    if error.is_transport() {
                        log_transport_end(&format!("client_id={id}"), &error);
                    } else {
                        log::warn!("business peer rejected: client_id={id}, error={error}");
                    }
                    true
                }
            };
            if close {
                // Revoke project routing before any later bridge can address it.
                if let Some(peer) = peers.remove(&id) {
                    peer.disconnected(&registry, hooks.as_ref());
                }
                if let Err(error) = endpoint.disconnect_client(id) {
                    log::debug!("Quinnet peer already closed: {error}");
                }
            }
        }
        BridgeStep::Idle
    });
    for peer in peers.values() {
        peer.disconnected(&registry, hooks.as_ref());
    }
    app.world_mut()
        .resource_mut::<QuinnetServer>()
        .stop_endpoint()
        .map_err(NetworkError::from_display)?;
    Ok(())
}
