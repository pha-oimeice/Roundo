//! Process-level composition between QUIC service callbacks and ECS domain pipes.
//!
//! Network hooks run on the networking runtime and only submit non-blocking ECS
//! messages. Two polling bridge threads drain ECS events back to bounded network
//! queues. Bridge forwarding is best-effort: rejected network admission is not
//! requeued by this adapter.

use log::{debug, info, warn};
use roundo_contracts::{
    ClientGameMessage, ClientResourceMessage, ConnectionId, ResourceCatalogFingerprint,
    ServerGameMessage, ServerResourceMessage, SessionId, StreamId, UserId, UserSession,
};
use roundo_ecs_entry::{PlayerControlCommand, PlayerControlEvent, PlayerControlServerIpc};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc,
};
use roundo_marionette::{ServerMarionetteCommand, ServerMarionetteEvent, ServerMarionetteIpc};
use roundo_networking::{
    HookFuture, PublicSession, ServerHooks, ServerNetwork, ServerNetworkConfig,
};
use roundo_presence::{PresenceServerEvent, PresenceServerIpc};
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
    /// Starts the network listener and both ECS-to-network bridge threads.
    ///
    /// The listener is ready before this returns. Database initialization, TLS
    /// setup, and listener startup happen as part of [`ServerNetwork::start`].
    ///
    /// # Panics
    ///
    /// Panics if configuration cannot be converted to a usable endpoint, the
    /// network server cannot start, or either bridge OS thread cannot be spawned.
    pub fn start(
        marionette_ipc: ServerMarionetteIpc,
        presence_ipc: PresenceServerIpc,
        local_coordinate_ipc: LocalCoordinateServerIpc,
        player_control_ipc: PlayerControlServerIpc,
        resource_fingerprint: ResourceCatalogFingerprint,
    ) -> Self {
        let config = network_config();
        debug!(
            "Starting network server: quic={}, certificate_directory={}",
            config.quic_address,
            config.certificate_directory.display()
        );
        let hooks = Arc::new(ServerHooksAdapter {
            marionette_ipc: marionette_ipc.clone(),
            local_coordinate_ipc: local_coordinate_ipc.clone(),
            player_control_ipc: player_control_ipc.clone(),
            resource_fingerprint,
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
                bridge_game_ecs_events(
                    marionette_ipc,
                    presence_ipc,
                    game_local_coordinate_ipc,
                    player_control_ipc,
                    game_network,
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
        Self { network, bridges }
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

/// Converts persistent process configuration into networking-service input.
///
/// Certificate paths remain relative to the process working directory unless
/// persistence supplied an absolute path.
fn network_config() -> ServerNetworkConfig {
    let network = &crate::config::SERVER_CONFIG.network;
    ServerNetworkConfig {
        quic_address: network.endpoint.get_quic_addr(),
        certificate_directory: PathBuf::from(&network.certificate_path).join("certs"),
        server_alternative_names: network.server_alternative_names.clone(),
        generate_self_signed_certificate: network.generate_self_signed_certificate,
        admission: roundo_networking::TransportAdmissionPolicy::default(),
    }
}

/// Non-blocking adapter from networking-runtime callbacks to ECS domain queues.
struct ServerHooksAdapter {
    marionette_ipc: ServerMarionetteIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
    player_control_ipc: PlayerControlServerIpc,
    resource_fingerprint: ResourceCatalogFingerprint,
}

impl ServerHooks for ServerHooksAdapter {
    /// Initializes the database before the listener reports readiness.
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

    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
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
                    user_id: UserId(session.owner_user_id),
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
                let _ = self
                    .player_control_ipc
                    .try_send(PlayerControlCommand::Submit {
                        connection_id,
                        controller_id,
                        input,
                    });
            }
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
    stop: Arc<AtomicBool>,
) {
    debug!("Started server game IPC bridge");
    run_polling_bridge(&stop, || {
        if let Some(event) = player_control_ipc.try_receive() {
            let (connection_id, message) = match event {
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
            network.send_to_connection(connection_id, StreamId::Stream0, message);
            return BridgeStep::Forwarded;
        }
        if let Some(ServerMarionetteEvent::CreatureMotionSnapshot {
            connection_id,
            snapshot,
        }) = marionette_ipc.try_receive()
        {
            network.send_to_connection(
                connection_id,
                StreamId::Stream0,
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
                    StreamId::Stream0,
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
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream0,
                    ServerGameMessage::PlayerState { state },
                );
            }
        }
        BridgeStep::Forwarded
    });
}

/// Drains coordinate events one at a time and projects them onto stream 1.
///
/// Network queue rejection drops that event after the networking layer logs it;
/// this bridge provides neither retry nor end-to-end delivery acknowledgment.
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
            LocalCoordinateServerEvent::RenderingAnchorSpawned {
                connection_id,
                anchor,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::RenderingAnchorSpawned { anchor },
                );
            }
            LocalCoordinateServerEvent::RenderingAnchorUpdated {
                connection_id,
                anchor,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::RenderingAnchorUpdated { anchor },
                );
            }
            LocalCoordinateServerEvent::RenderingAnchorDespawned {
                connection_id,
                anchor_id,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::RenderingAnchorDespawned { anchor_id },
                );
            }
            LocalCoordinateServerEvent::PredictionAnchorSpawned {
                connection_id,
                anchor,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::PredictionAnchorSpawned { anchor },
                );
            }
            LocalCoordinateServerEvent::PredictionAnchorUpdated {
                connection_id,
                anchor,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::PredictionAnchorUpdated { anchor },
                );
            }
            LocalCoordinateServerEvent::PredictionAnchorDespawned {
                connection_id,
                anchor_id,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::PredictionAnchorDespawned { anchor_id },
                );
            }
            LocalCoordinateServerEvent::EnvironmentOverrides {
                connection_id,
                overrides,
            } => {
                network.send_to_connection(
                    connection_id,
                    StreamId::Stream1,
                    ServerResourceMessage::VirtualChunkEnvironmentOverrides { overrides },
                );
            }
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

#[cfg(test)]
mod tests {
    use super::peer_identity_key;

    #[test]
    fn peer_address_becomes_only_an_opaque_external_identity_key() {
        let address = "127.0.0.1:4111".parse().unwrap();
        assert_eq!(peer_identity_key(address), "127.0.0.1:4111");
    }
}
