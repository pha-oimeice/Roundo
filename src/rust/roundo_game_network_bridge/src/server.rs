//! Process-level composition between QUIC service callbacks and ECS domain pipes.
//!
//! Network hooks run on the networking runtime and only submit non-blocking ECS
//! messages. Two polling bridge threads drain ECS events back to bounded network
//! queues. Game forwarding is best-effort; resource forwarding retains saturated
//! sends in an ordered, bounded staging window until admission or disconnection.

mod resource;
use resource::bridge_resource_ecs_events;

use log::{debug, info, warn};
#[cfg(feature = "client-test")]
use roundo_contracts::control_handoff::ControlRequest;
use roundo_contracts::{
    ClientGameMessage, ClientResourceMessage, ConnectionId, CreatureMotionSnapshot,
    ResourceCatalogFingerprint, ServerGameMessage, ServerResourceMessage, SessionId, UserSession,
    control_handoff::DrainBarrier,
};
use roundo_ecs_entry::{PlayerControlCommand, PlayerControlEvent, PlayerControlServerIpc};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc,
};
use roundo_marionette::{ServerMarionetteCommand, ServerMarionetteEvent, ServerMarionetteIpc};
use roundo_networking::{
    HookFuture, PublicSession, ServerAddresses, ServerHooks, ServerNetwork, ServerNetworkConfig,
};
use roundo_presence::{PresenceServerEvent, PresenceServerIpc};
use roundo_toolbox::{BridgeStep, BridgeThreadGroup, run_polling_bridge};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};

/// Bounded non-consuming authenticated ingress trace behind `client-test`.
#[cfg(feature = "client-test")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestControlIngress {
    Submit {
        connection: ConnectionId,
        controller: roundo_contracts::ControllerId,
        sequence: u64,
    },
    ControlRequest {
        connection: ConnectionId,
        request: ControlRequest,
    },
}

/// Owns the server network and both ECS bridge adapters for the complete Bevy
/// application lifetime. The bridges borrow their sending capability through
/// `Arc`, but this Facade is the explicit runtime owner: shutdown first stops
/// and joins the bridges, then joins the network worker.
pub struct ServerNetworkRuntime {
    network: Arc<ServerNetwork>,
    bridges: BridgeThreadGroup,
    connected: Mutex<Receiver<ConnectionId>>,
    drain_barriers: Mutex<Receiver<(ConnectionId, DrainBarrier)>>,
    drain_barrier_notice_overflow: Arc<AtomicBool>,
    snapshots: Mutex<Receiver<(ConnectionId, CreatureMotionSnapshot)>>,
    #[cfg(feature = "client-test")]
    control_ingress: Mutex<Receiver<TestControlIngress>>,
    #[cfg(feature = "client-test")]
    control_ingress_overflow: Arc<AtomicBool>,
}

impl ServerNetworkRuntime {
    /// Starts the network listener and both ECS-to-network bridge threads.
    ///
    /// The listener is ready before this returns. Provider initialization, TLS
    /// setup, and listener startup happen as part of [`ServerNetwork::start`].
    ///
    /// # Panics
    ///
    /// Panics if configuration cannot be converted to a usable endpoint, the
    /// network server cannot start, or either bridge OS thread cannot be spawned.
    pub fn start(
        config: ServerNetworkConfig,
        session_provider: Arc<dyn ServerSessionProvider>,
        marionette_ipc: ServerMarionetteIpc,
        presence_ipc: PresenceServerIpc,
        local_coordinate_ipc: LocalCoordinateServerIpc,
        player_control_ipc: PlayerControlServerIpc,
        resource_fingerprint: ResourceCatalogFingerprint,
    ) -> Self {
        debug!(
            "Starting network server: quic={}, certificate_directory={}",
            config.quic_address,
            config.certificate_directory.display()
        );
        let (hooks, connected) = ServerHooksAdapter::new(
            session_provider,
            marionette_ipc.clone(),
            local_coordinate_ipc.clone(),
            player_control_ipc.clone(),
            resource_fingerprint,
        );
        let (drain_barrier_sender, drain_barriers) = mpsc::sync_channel(64);
        let drain_barrier_notice_overflow = Arc::new(AtomicBool::new(false));
        let hooks = hooks.with_drain_barrier_notices(
            drain_barrier_sender,
            Arc::clone(&drain_barrier_notice_overflow),
        );
        #[cfg(feature = "client-test")]
        let (control_ingress_sender, control_ingress) = mpsc::sync_channel(128);
        #[cfg(feature = "client-test")]
        let control_ingress_overflow = Arc::new(AtomicBool::new(false));
        #[cfg(feature = "client-test")]
        let hooks = hooks.with_control_ingress_notices(
            control_ingress_sender,
            Arc::clone(&control_ingress_overflow),
        );
        let network = Arc::new(
            ServerNetwork::start(config, Arc::new(hooks))
                .unwrap_or_else(|error| panic!("failed to start network server: {error}")),
        );
        let addresses = network.addresses();
        info!("Network server is ready: quic={}", addresses.quic_address);

        let (snapshot_sender, snapshots) = mpsc::sync_channel(64);
        let mut bridges = BridgeThreadGroup::new();
        let game_network = Arc::clone(&network);
        let game_local_coordinate_ipc = local_coordinate_ipc.clone();
        bridges
            .spawn("roundo-server-game-bridge", move |stop| {
                bridge_game_ecs_events(
                    marionette_ipc,
                    presence_ipc,
                    game_local_coordinate_ipc,
                    player_control_ipc,
                    game_network,
                    snapshot_sender,
                    stop,
                )
            })
            .unwrap_or_else(|error| panic!("failed to start server game bridge: {error}"));
        let resource_network = Arc::clone(&network);
        bridges
            .spawn("roundo-server-resource-bridge", move |stop| {
                bridge_resource_ecs_events(local_coordinate_ipc, resource_network, stop)
            })
            .unwrap_or_else(|error| panic!("failed to start server resource bridge: {error}"));
        Self {
            network,
            bridges,
            connected: Mutex::new(connected),
            drain_barriers: Mutex::new(drain_barriers),
            drain_barrier_notice_overflow,
            snapshots: Mutex::new(snapshots),
            #[cfg(feature = "client-test")]
            control_ingress: Mutex::new(control_ingress),
            #[cfg(feature = "client-test")]
            control_ingress_overflow,
        }
    }

    /// Returns the bound listener addresses (including an ephemeral port when configured).
    pub fn addresses(&self) -> ServerAddresses {
        self.network.addresses()
    }

    /// Observes the next authenticated transport connection ID without consuming
    /// PlayerControlEvent::Connected from the ECS bridge. Notices are diagnostic:
    /// if the bounded notice queue is full they are dropped without blocking QUIC.
    pub fn try_receive_connected(&self) -> Option<ConnectionId> {
        match self
            .connected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_recv()
        {
            Ok(id) => Some(id),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Non-consuming, bounded observation of a committed ECS snapshot. This
    /// diagnostic queue never drains the network bridge's snapshot delivery.
    pub fn try_receive_snapshot(&self) -> Option<(ConnectionId, CreatureMotionSnapshot)> {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_recv()
            .ok()
    }

    /// Non-consuming notice from the authenticated server hook. Overflow is
    /// sticky and reported as an error rather than mistaken for no barrier.
    pub fn try_receive_drain_barrier(
        &self,
    ) -> Result<Option<(ConnectionId, DrainBarrier)>, &'static str> {
        if self.drain_barrier_notice_overflow.load(Ordering::Acquire) {
            return Err("drain barrier diagnostic queue overflowed");
        }
        let notice = self
            .drain_barriers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_recv()
            .ok();
        if self.drain_barrier_notice_overflow.load(Ordering::Acquire) {
            return Err("drain barrier diagnostic queue overflowed");
        }
        Ok(notice)
    }

    /// Observes authenticated, successfully ECS-enqueued Submit/control-request messages
    /// in callback order; overflow is sticky, never interpreted as no ingress.
    #[cfg(feature = "client-test")]
    pub fn try_receive_test_control_ingress(
        &self,
    ) -> Result<Option<TestControlIngress>, &'static str> {
        if self.control_ingress_overflow.load(Ordering::Acquire) {
            return Err("control ingress notice overflow");
        }
        let notice = self
            .control_ingress
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_recv()
            .ok();
        if self.control_ingress_overflow.load(Ordering::Acquire) {
            return Err("control ingress notice overflow");
        }
        Ok(notice)
    }

    /// Headless integration diagnostic: deliver an authentic snapshot DTO over
    /// this authenticated connection's production game stream, never a gate event.
    #[cfg(feature = "client-test")]
    pub fn send_test_snapshot(
        &self,
        connection: ConnectionId,
        snapshot: CreatureMotionSnapshot,
    ) -> bool {
        self.network.send_to_connection(
            connection,
            ServerGameMessage::CreatureMotionSnapshot { snapshot },
        )
    }

    /// Signals and joins both bridge adapters before stopping the network worker.
    ///
    /// Calling it more than once is harmless. This blocks the current OS thread
    /// and relies on bridge workers cooperatively observing their stop flags.
    pub fn shutdown(&mut self) {
        self.bridges.shutdown();
        self.network.shutdown();
    }
}

impl Drop for ServerNetworkRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Temporary identity adapter: peer address is an opaque external key, never a PlayerId.
fn peer_identity_key(peer_address: std::net::SocketAddr) -> String {
    peer_address.to_string()
}

/// Process-owned session lookups; the bridge never accesses the database directly.
pub trait ServerSessionProvider: Send + Sync + 'static {
    fn initialize(&self) -> HookFuture<()>;
    fn public_session(&self) -> HookFuture<PublicSession>;
    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool>;
}

/// Non-blocking adapter from networking-runtime callbacks to ECS domain queues.
pub struct ServerHooksAdapter {
    session_provider: Arc<dyn ServerSessionProvider>,
    connected_sender: SyncSender<ConnectionId>,
    drain_barrier_notices: Option<(SyncSender<(ConnectionId, DrainBarrier)>, Arc<AtomicBool>)>,
    #[cfg(feature = "client-test")]
    control_ingress_notices: Option<(SyncSender<TestControlIngress>, Arc<AtomicBool>)>,
    marionette_ipc: ServerMarionetteIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
    player_control_ipc: PlayerControlServerIpc,
    resource_fingerprint: ResourceCatalogFingerprint,
}

impl ServerHooksAdapter {
    /// Builds the same hook used by the running server; the receiver observes
    /// connection identities without draining any ECS events.
    pub fn new(
        session_provider: Arc<dyn ServerSessionProvider>,
        marionette_ipc: ServerMarionetteIpc,
        local_coordinate_ipc: LocalCoordinateServerIpc,
        player_control_ipc: PlayerControlServerIpc,
        resource_fingerprint: ResourceCatalogFingerprint,
    ) -> (Self, Receiver<ConnectionId>) {
        let (connected_sender, receiver) = mpsc::sync_channel(64);
        (
            Self {
                session_provider,
                connected_sender,
                drain_barrier_notices: None,
                #[cfg(feature = "client-test")]
                control_ingress_notices: None,
                marionette_ipc,
                local_coordinate_ipc,
                player_control_ipc,
                resource_fingerprint,
            },
            receiver,
        )
    }

    #[cfg(feature = "client-test")]
    fn with_control_ingress_notices(
        mut self,
        sender: SyncSender<TestControlIngress>,
        overflow: Arc<AtomicBool>,
    ) -> Self {
        self.control_ingress_notices = Some((sender, overflow));
        self
    }

    #[cfg(feature = "client-test")]
    fn notice_test_control_ingress(&self, notice: TestControlIngress) {
        if let Some((sender, overflow)) = &self.control_ingress_notices {
            if sender.try_send(notice).is_err() {
                overflow.store(true, Ordering::Release);
            }
        }
    }

    fn with_drain_barrier_notices(
        mut self,
        sender: SyncSender<(ConnectionId, DrainBarrier)>,
        overflow: Arc<AtomicBool>,
    ) -> Self {
        self.drain_barrier_notices = Some((sender, overflow));
        self
    }
}

impl ServerHooks for ServerHooksAdapter {
    fn initialize(&self) -> HookFuture<()> {
        self.session_provider.initialize()
    }

    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
    }

    fn public_session(&self) -> HookFuture<PublicSession> {
        self.session_provider.public_session()
    }

    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool> {
        self.session_provider.session_is_open(session_id)
    }

    fn on_session_connected(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        peer_address: std::net::SocketAddr,
    ) {
        debug!(
            "Forwarding session connection to ECS: connection_id={}, user_id={}, session_id={}",
            connection_id.0, user_session.user_id.0, user_session.session_id.0
        );
        // Diagnostic notice is bounded and never blocks the transport callback.
        let _ = self.connected_sender.try_send(connection_id);
        if self
            .player_control_ipc
            .try_send(PlayerControlCommand::Connected {
                connection_id,
                external_identity: peer_identity_key(peer_address),
            })
            .is_err()
        {
            warn!(
                "Failed to resolve player identity: connection_id={}",
                connection_id.0
            );
        }
    }

    fn on_session_disconnected(
        &self,
        connection_id: ConnectionId,
        _: UserSession,
        _: std::net::SocketAddr,
    ) {
        if self
            .player_control_ipc
            .try_send(PlayerControlCommand::Disconnected { connection_id })
            .is_err()
        {
            warn!(
                "Failed to unbind player identity: connection_id={}",
                connection_id.0
            );
        }
        if self
            .local_coordinate_ipc
            .try_send(LocalCoordinateServerCommand::UnsubscribePlayer { connection_id })
            .is_err()
        {
            warn!(
                "Failed to remove player voxel subscription: connection_id={}",
                connection_id.0
            );
        }
    }

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        _: UserSession,
        message: ClientGameMessage,
    ) {
        match message {
            ClientGameMessage::Inventory(request) => {
                let _ = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::Inventory {
                        connection_id,
                        request,
                    });
            }
            ClientGameMessage::Debug(request) => {
                let _ = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::Debug {
                        connection_id,
                        request,
                    });
            }
            ClientGameMessage::RequestPlayerControllerAccess => {
                let _ = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::RequestAccess { connection_id });
            }
            ClientGameMessage::AcquireController { controller_id } => {
                let _ = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::Acquire {
                        connection_id,
                        controller_id,
                    });
            }
            ClientGameMessage::ReleaseController { controller_id } => {
                let _ = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::Release {
                        connection_id,
                        controller_id,
                    });
            }
            ClientGameMessage::SubmitControllerInput {
                controller_id,
                input,
            } => {
                #[cfg(feature = "client-test")]
                let sequence = match &input {
                    roundo_contracts::DirectedControllerInput::Movement3D(command) => {
                        command.sequence
                    }
                    roundo_contracts::DirectedControllerInput::Gaze(command) => command.sequence,
                    roundo_contracts::DirectedControllerInput::SetGaze(command) => command.sequence,
                };
                let queued = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::Submit {
                        connection_id,
                        controller_id,
                        input,
                    })
                    .is_ok();
                #[cfg(feature = "client-test")]
                if queued {
                    self.notice_test_control_ingress(TestControlIngress::Submit {
                        connection: connection_id,
                        controller: controller_id,
                        sequence,
                    });
                }
                #[cfg(not(feature = "client-test"))]
                let _ = queued;
            }
            ClientGameMessage::ControlRequest(request) => {
                if self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::TicketedRequest {
                        connection_id,
                        request,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to dispatch ticketed request: connection_id={}",
                        connection_id.0
                    );
                } else {
                    #[cfg(feature = "client-test")]
                    self.notice_test_control_ingress(TestControlIngress::ControlRequest {
                        connection: connection_id,
                        request,
                    });
                }
            }
            ClientGameMessage::Ready(ready) => {
                if self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::TicketedReady {
                        connection_id,
                        ready,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to dispatch ticketed Ready: connection_id={}",
                        connection_id.0
                    );
                }
            }
            ClientGameMessage::DrainBarrier(barrier) => {
                // The callback's authenticated connection, never a client field.
                if self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::DrainBarrier {
                        connection_id,
                        barrier,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to forward drain barrier to ECS: connection_id={}",
                        connection_id.0
                    );
                }
                // Independent of ECS admission: this observes actual wire arrival,
                // and never consumes or changes the domain command.
                if let Some((sender, overflow)) = &self.drain_barrier_notices {
                    if sender.try_send((connection_id, barrier)).is_err() {
                        overflow.store(true, Ordering::Release);
                    }
                }
            }
            ClientGameMessage::UsePlayerController { command } => match command {
                roundo_contracts::PlayerControllerCommand::Movement3D(_)
                | roundo_contracts::PlayerControllerCommand::Gaze(_)
                | roundo_contracts::PlayerControllerCommand::SetGaze(_) => {
                    warn!(
                        "Dropped legacy untargeted creature input: connection_id={}",
                        connection_id.0
                    );
                }
                command => {
                    if self
                        .marionette_ipc
                        .try_send(ServerMarionetteCommand::UsePlayerController {
                            connection_id,
                            command,
                        })
                        .is_err()
                    {
                        warn!(
                            "Failed to forward legacy player controller command to ECS: connection_id={}",
                            connection_id.0
                        );
                    }
                }
            },
        }
    }

    fn on_client_resource_message(
        &self,
        connection_id: ConnectionId,
        _: UserSession,
        message: ClientResourceMessage,
    ) {
        match message {
            ClientResourceMessage::SetChunkViewDistance { chunks } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateServerCommand::SetChunkViewDistance {
                        connection_id,
                        chunks,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to forward Chunk view distance to ECS: connection_id={}, chunks={chunks}, reason=local_coordinate_channel_closed",
                        connection_id.0
                    );
                }
            }
        }
    }
}

/// Drains presence events one at a time and projects them onto stream 0.
///
/// Player-join events instead establish local-coordinate subscription state.
/// Failed ECS or network admission is logged by the relevant adapter and is not
/// retried here.
fn bridge_game_ecs_events(
    marionette_ipc: ServerMarionetteIpc,
    presence_ipc: PresenceServerIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
    player_control_ipc: PlayerControlServerIpc,
    network: Arc<ServerNetwork>,
    snapshot_sender: SyncSender<(ConnectionId, CreatureMotionSnapshot)>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started server game IPC bridge");
    run_polling_bridge(&stop, || {
        if let Some(event) = player_control_ipc.try_receive() {
            let (connection_id, message) = match event {
                PlayerControlEvent::Inventory {
                    connection_id,
                    reply,
                } => (connection_id, ServerGameMessage::Inventory(reply)),
                PlayerControlEvent::Debug {
                    connection_id,
                    reply,
                } => (connection_id, ServerGameMessage::Debug(reply)),
                // Prefix observation stays server-local; ticket terminals are forwarded separately.
                PlayerControlEvent::PrefixAck(_) => return BridgeStep::Forwarded,
                PlayerControlEvent::TicketedTerminal {
                    connection_id,
                    terminal,
                } => (connection_id, ServerGameMessage::ControlTerminal(terminal)),
                PlayerControlEvent::Preparing {
                    connection_id,
                    preparing,
                } => (
                    connection_id,
                    ServerGameMessage::ControlPreparing(preparing),
                ),
                PlayerControlEvent::Freeze {
                    connection_id,
                    freeze,
                } => {
                    let queued = network
                        .send_to_connection(connection_id, ServerGameMessage::Freeze(freeze));
                    if player_control_ipc
                        .try_send(PlayerControlCommand::FreezeDispatchReported {
                            connection_id,
                            request_id: freeze.request_id,
                            queued,
                        })
                        .is_err()
                    {
                        warn!("Failed to report Freeze dispatch; ECS deadline will fail closed");
                    }
                    return BridgeStep::Forwarded;
                }
                PlayerControlEvent::Connected {
                    connection_id,
                    player_id,
                } => {
                    if local_coordinate_ipc
                        .try_send(LocalCoordinateServerCommand::SubscribePlayer {
                            connection_id,
                            player_id,
                        })
                        .is_err()
                    {
                        warn!(
                            "Failed to subscribe Player to local-coordinate updates: connection_id={}, player_id={}",
                            connection_id.0, player_id.0
                        );
                    }
                    return BridgeStep::Forwarded;
                }
                PlayerControlEvent::AccessSnapshot {
                    connection_id,
                    snapshot,
                } => (
                    connection_id,
                    ServerGameMessage::PlayerControllerAccessSnapshot { snapshot },
                ),
                PlayerControlEvent::Acquired {
                    connection_id,
                    controller_id,
                } => (
                    connection_id,
                    ServerGameMessage::ControllerAcquired { controller_id },
                ),
                PlayerControlEvent::Released {
                    connection_id,
                    controller_id,
                } => (
                    connection_id,
                    ServerGameMessage::ControllerReleased { controller_id },
                ),
                PlayerControlEvent::Rejected {
                    connection_id,
                    operation,
                    error,
                } => (
                    connection_id,
                    ServerGameMessage::ControllerOperationRejected { operation, error },
                ),
            };
            network.send_to_connection(connection_id, message);
            return BridgeStep::Forwarded;
        }
        if let Some(ServerMarionetteEvent::CreatureMotionSnapshot {
            connection_id,
            snapshot,
        }) = marionette_ipc.try_receive()
        {
            let _ = snapshot_sender.try_send((connection_id, snapshot));
            network.send_to_connection(
                connection_id,
                ServerGameMessage::CreatureMotionSnapshot { snapshot },
            );
            return BridgeStep::Forwarded;
        }
        let Some(event) = presence_ipc.try_receive() else {
            return BridgeStep::Idle;
        };
        match event {
            PresenceServerEvent::Snapshot {
                connection_id,
                snapshot,
            } => {
                network.send_to_connection(
                    connection_id,
                    ServerGameMessage::PresenceSnapshot { snapshot },
                );
            }
            PresenceServerEvent::PlayerJoined {
                connection_id,
                player_id,
            } => {
                if local_coordinate_ipc
                    .try_send(LocalCoordinateServerCommand::SubscribePlayer {
                        connection_id,
                        player_id,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to subscribe player to local-coordinate updates: connection_id={}, player_id={}",
                        connection_id.0, player_id.0
                    );
                }
            }
            PresenceServerEvent::PlayerStateChanged {
                connection_id,
                state,
            } => {
                network.send_to_connection(connection_id, ServerGameMessage::PlayerState { state });
            }
        }
        BridgeStep::Forwarded
    });
}
