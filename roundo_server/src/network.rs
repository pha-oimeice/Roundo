use log::{debug, info, warn};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc,
};
use roundo_marionette::{ServerMarionetteCommand, ServerMarionetteIpc};
use roundo_networking::{
    ClientGameMessage, ClientResourceMessage, ConnectionId, HookFuture, PublicSession, ServerHooks,
    ServerNetwork, ServerNetworkConfig, ServerResourceMessage, SessionId, StreamId, UserSession,
};
use roundo_presence::{PresenceServerCommand, PresenceServerEvent, PresenceServerIpc};
use roundo_toolbox::{BridgeStep, BridgeThreadGroup, run_polling_bridge};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

/// Owns the server network and both ECS bridge adapters for the complete Bevy
/// application lifetime. The bridges borrow their sending capability through
/// `Arc`, but this Facade is the explicit runtime owner: shutdown first stops
/// and joins the bridges, then joins the network worker.
pub struct ServerNetworkRuntime {
    network: Arc<ServerNetwork>,
    bridges: BridgeThreadGroup,
}

impl ServerNetworkRuntime {
    pub fn start(
        marionette_ipc: ServerMarionetteIpc,
        presence_ipc: PresenceServerIpc,
        local_coordinate_ipc: LocalCoordinateServerIpc,
    ) -> Self {
        let config = network_config();
        debug!(
            "Starting network server: quic={}, certificate_directory={}",
            config.quic_address,
            config.certificate_directory.display()
        );
        let hooks = Arc::new(ServerHooksAdapter {
            marionette_ipc,
            presence_ipc: presence_ipc.clone(),
            local_coordinate_ipc: local_coordinate_ipc.clone(),
        });
        let network = Arc::new(
            ServerNetwork::start(config, hooks)
                .unwrap_or_else(|error| panic!("failed to start network server: {error}")),
        );
        let addresses = network.addresses();
        info!("Network server is ready: quic={}", addresses.quic_address);

        let mut bridges = BridgeThreadGroup::new();
        let game_network = Arc::clone(&network);
        let game_local_coordinate_ipc = local_coordinate_ipc.clone();
        bridges
            .spawn("roundo-server-game-bridge", move |stop| {
                bridge_game_ecs_events(presence_ipc, game_local_coordinate_ipc, game_network, stop)
            })
            .unwrap_or_else(|error| panic!("failed to start server game bridge: {error}"));
        let resource_network = Arc::clone(&network);
        bridges
            .spawn("roundo-server-resource-bridge", move |stop| {
                bridge_resource_ecs_events(local_coordinate_ipc, resource_network, stop)
            })
            .unwrap_or_else(|error| panic!("failed to start server resource bridge: {error}"));
        Self { network, bridges }
    }

    /// Deterministically stops bridge adapters before releasing the network.
    /// Calling it more than once is harmless.
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

fn network_config() -> ServerNetworkConfig {
    let network = &crate::config::SERVER_CONFIG.network;
    ServerNetworkConfig {
        quic_address: network.endpoint.get_quic_addr(),
        certificate_directory: PathBuf::from(&network.certificate_path).join("certs"),
        server_alternative_names: network.server_alternative_names.clone(),
        generate_self_signed_certificate: network.generate_self_signed_certificate,
    }
}

struct ServerHooksAdapter {
    marionette_ipc: ServerMarionetteIpc,
    presence_ipc: PresenceServerIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
}

impl ServerHooks for ServerHooksAdapter {
    fn initialize(&self) -> HookFuture<()> {
        Box::pin(async {
            debug!("Initializing database before accepting network connections");
            let result = crate::my_db::initialize_database()
                .await
                .map_err(|error| error.to_string());
            if result.is_ok() {
                debug!("Database initialization completed");
            }
            result
        })
    }

    fn public_session(&self) -> HookFuture<PublicSession> {
        Box::pin(async {
            debug!("Resolving public session for connection ticket request");
            let session = crate::my_db::game_session::public_session()
                .await
                .map_err(|error| error.to_string())?;
            debug!(
                "Resolved public session: user_id={}, session_id={}",
                session.owner_user_id, session.id
            );
            Ok(PublicSession {
                user_session: UserSession {
                    user_id: roundo_networking::UserId(session.owner_user_id),
                    session_id: SessionId(session.id),
                },
                session_name: session.name,
            })
        })
    }

    fn session_is_open(&self, session_id: SessionId) -> HookFuture<bool> {
        Box::pin(async move {
            let is_open = crate::my_db::game_session::is_session_open(session_id.0)
                .await
                .map_err(|error| error.to_string())?;
            debug!("Validated game session {}: open={is_open}", session_id.0);
            Ok(is_open)
        })
    }

    fn on_session_connected(&self, connection_id: ConnectionId, user_session: UserSession) {
        debug!(
            "Forwarding session connection to ECS: connection_id={}, user_id={}, session_id={}",
            connection_id.0, user_session.user_id.0, user_session.session_id.0
        );
        if self
            .presence_ipc
            .try_send(PresenceServerCommand::Connect { connection_id })
            .is_err()
        {
            warn!(
                "Failed to create player presence: connection_id={}",
                connection_id.0
            );
        }
    }

    fn on_session_disconnected(&self, connection_id: ConnectionId, _: UserSession) {
        if self
            .presence_ipc
            .try_send(PresenceServerCommand::Disconnect { connection_id })
            .is_err()
        {
            warn!(
                "Failed to remove player presence: connection_id={}",
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
            ClientGameMessage::UsePlayerController { command } => {
                if self
                    .marionette_ipc
                    .try_send(ServerMarionetteCommand::UsePlayerController {
                        connection_id,
                        command,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to forward player controller command to ECS: connection_id={}",
                        connection_id.0
                    );
                }
            }
        }
    }

    fn on_client_resource_message(
        &self,
        connection_id: ConnectionId,
        _: UserSession,
        message: ClientResourceMessage,
    ) {
        match message {
            ClientResourceMessage::RequestLocalCoordinateChunks { chunks } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateServerCommand::RequestChunks {
                        connection_id,
                        chunks,
                    })
                    .is_err()
                {
                    warn!(
                        "Failed to forward chunk request to ECS: connection_id={}",
                        connection_id.0
                    );
                }
            }
            ClientResourceMessage::SetChunkViewDistance { chunks } => {
                let _ = self.local_coordinate_ipc.try_send(
                    LocalCoordinateServerCommand::SetChunkViewDistance {
                        connection_id,
                        chunks,
                    },
                );
            }
        }
    }
}

fn bridge_game_ecs_events(
    presence_ipc: PresenceServerIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
    network: Arc<ServerNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started server game IPC bridge");
    run_polling_bridge(&stop, || {
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
                    StreamId::Stream0,
                    roundo_networking::ServerGameMessage::PresenceSnapshot { snapshot },
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
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream0,
                    roundo_networking::ServerGameMessage::PlayerState { state },
                );
            }
        }
        BridgeStep::Forwarded
    });
}

fn bridge_resource_ecs_events(
    local_coordinate_ipc: LocalCoordinateServerIpc,
    network: Arc<ServerNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started server resource IPC bridge");
    run_polling_bridge(&stop, || {
        let Some(event) = local_coordinate_ipc.try_receive() else {
            return BridgeStep::Idle;
        };
        match event {
            LocalCoordinateServerEvent::Spawned {
                connection_id,
                local_coordinate_id,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::LocalCoordinateSpawned {
                        local_coordinate_id,
                    },
                );
            }
            LocalCoordinateServerEvent::Despawned {
                connection_id,
                local_coordinate_id,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::LocalCoordinateDespawned {
                        local_coordinate_id,
                    },
                );
            }
            LocalCoordinateServerEvent::ChunkLoaded {
                connection_id,
                chunk,
                payload,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::LocalCoordinateChunk {
                        chunk,
                        edge_length: CHUNK_EDGE_LENGTH as u16,
                        svo: payload,
                    },
                );
            }
            LocalCoordinateServerEvent::ChunkVersions {
                connection_id,
                chunks,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::LocalCoordinateChunkVersions { chunks },
                );
            }
            LocalCoordinateServerEvent::ChunkUnloaded {
                connection_id,
                local_coordinate_id,
                coordinate,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::LocalCoordinateChunkUnloaded {
                        local_coordinate_id,
                        coordinate,
                    },
                );
            }
        }
        BridgeStep::Forwarded
    });
}
