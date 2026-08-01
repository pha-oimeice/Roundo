use log::{debug, warn};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc,
};
use roundo_marionette::{ServerMarionetteCommand, ServerMarionetteEvent, ServerMarionetteIpc};
use roundo_networking::{
    ClientGameMessage, ConnectionId, HookFuture, PublicSession, ServerHooks, ServerNetwork,
    ServerNetworkConfig, SessionId, UserSession,
};
use roundo_presence::{PresenceServerCommand, PresenceServerEvent, PresenceServerIpc};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub fn start_server(
    marionette_ipc: ServerMarionetteIpc,
    presence_ipc: PresenceServerIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
) {
    let config = network_config();
    debug!(
        "Starting network server: game={}, public={}, certificate_directory={}",
        config.game_address,
        config.public_address,
        config.certificate_directory.display()
    );
    let hooks = Arc::new(ServerHooksAdapter {
        marionette_ipc: marionette_ipc.clone(),
        presence_ipc: presence_ipc.clone(),
        local_coordinate_ipc: local_coordinate_ipc.clone(),
    });
    let network = ServerNetwork::start(config, hooks)
        .unwrap_or_else(|error| panic!("failed to start network server: {error}"));
    let addresses = network.addresses();
    debug!(
        "Network server is ready: game={}, public={}",
        addresses.game_address, addresses.public_address
    );

    std::thread::spawn(move || {
        bridge_ecs_events(marionette_ipc, presence_ipc, local_coordinate_ipc, network)
    });
}

fn network_config() -> ServerNetworkConfig {
    let network = &crate::config::SERVER_CONFIG.network;
    ServerNetworkConfig {
        game_address: network.endpoint.get_game_addr(),
        public_address: network.endpoint.get_https_addr(),
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
            .marionette_ipc
            .try_send(ServerMarionetteCommand::SessionConnected {
                connection_id,
                user_session,
            })
            .is_err()
        {
            warn!("Failed to forward session connection to ECS");
        }
        if self
            .presence_ipc
            .try_send(PresenceServerCommand::Connect { connection_id })
            .is_err()
        {
            warn!("Failed to create player presence for connection");
        }
    }

    fn on_session_disconnected(&self, connection_id: ConnectionId, _: UserSession) {
        if self
            .presence_ipc
            .try_send(PresenceServerCommand::Disconnect { connection_id })
            .is_err()
        {
            warn!("Failed to remove player presence for connection");
        }
        if self
            .local_coordinate_ipc
            .try_send(LocalCoordinateServerCommand::UnsubscribePlayer { connection_id })
            .is_err()
        {
            warn!("Failed to remove player voxel subscription for connection");
        }
    }

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientGameMessage,
    ) {
        let command = match message {
            ClientGameMessage::UpdatePlayerPosition { translation } => {
                if self
                    .presence_ipc
                    .try_send(PresenceServerCommand::UpdatePosition {
                        connection_id,
                        translation,
                    })
                    .is_err()
                {
                    warn!("Failed to forward player position to ECS");
                }
                return;
            }
            ClientGameMessage::RequestController { controller_id } => {
                debug!(
                    "Routing controller bind request: connection_id={}, controller_id={}",
                    connection_id.0, controller_id.0
                );
                ServerMarionetteCommand::RequestBinding {
                    connection_id,
                    user_session,
                    controller_id,
                }
            }
            ClientGameMessage::ReleaseController { controller_id } => {
                debug!(
                    "Routing controller release request: user_id={}, session_id={}, controller_id={}",
                    user_session.user_id.0, user_session.session_id.0, controller_id.0
                );
                ServerMarionetteCommand::ReleaseBinding {
                    user_session,
                    controller_id,
                }
            }
            ClientGameMessage::ControllerInput {
                controller_id,
                input,
            } => {
                log::trace!(
                    "Routing controller input: connection_id={}, controller_id={}",
                    connection_id.0,
                    controller_id.0
                );
                ServerMarionetteCommand::SubmitInput {
                    connection_id,
                    user_session,
                    controller_id,
                    input,
                }
            }
        };
        if self.marionette_ipc.try_send(command).is_err() {
            warn!("Failed to forward client game message to ECS");
        }
    }
}

fn bridge_ecs_events(
    marionette_ipc: ServerMarionetteIpc,
    presence_ipc: PresenceServerIpc,
    local_coordinate_ipc: LocalCoordinateServerIpc,
    network: ServerNetwork,
) {
    debug!("Started ECS-to-network event bridge");
    loop {
        let mut handled_event = false;

        while let Some(event) = marionette_ipc.try_receive() {
            handled_event = true;
            match event {
                ServerMarionetteEvent::ControllerGranted {
                    connection_id,
                    controller,
                } => {
                    debug!(
                        "Sending controller grant: connection_id={}, controller_id={}",
                        connection_id.0, controller.controller_id.0
                    );
                    if !network.send_to_connection(
                        connection_id,
                        roundo_networking::ServerGameMessage::ControllerGranted { controller },
                    ) {
                        warn!(
                            "Dropped controller grant because connection {} is unavailable",
                            connection_id.0
                        );
                    }
                }
                ServerMarionetteEvent::ControllerRevoked {
                    user_session,
                    controller_id,
                } => {
                    debug!(
                        "Broadcasting controller revocation: user_id={}, session_id={}, controller_id={}",
                        user_session.user_id.0, user_session.session_id.0, controller_id.0
                    );
                    network.send_to_session(
                        user_session,
                        roundo_networking::ServerGameMessage::ControllerRevoked { controller_id },
                    );
                }
                ServerMarionetteEvent::ViewCameraState {
                    user_session,
                    controller_id,
                    state,
                } => {
                    log::trace!(
                        "Broadcasting view camera state: user_id={}, session_id={}, controller_id={}",
                        user_session.user_id.0,
                        user_session.session_id.0,
                        controller_id.0
                    );
                    network.send_to_session(
                        user_session,
                        roundo_networking::ServerGameMessage::ViewCameraState {
                            controller_id,
                            state,
                        },
                    );
                }
            }
        }

        while let Some(event) = presence_ipc.try_receive() {
            handled_event = true;
            match event {
                PresenceServerEvent::Snapshot {
                    connection_id,
                    snapshot,
                } => {
                    network.send_to_connection(
                        connection_id,
                        roundo_networking::ServerGameMessage::PresenceSnapshot { snapshot },
                    );
                }
                PresenceServerEvent::PlayerJoined {
                    connection_id,
                    player_id,
                } => {
                    let _ = local_coordinate_ipc.try_send(
                        LocalCoordinateServerCommand::SubscribePlayer {
                            connection_id,
                            player_id,
                        },
                    );
                }
            }
        }

        while let Some(event) = local_coordinate_ipc.try_receive() {
            handled_event = true;
            match event {
                LocalCoordinateServerEvent::Spawned {
                    connection_id,
                    local_coordinate_id,
                } => {
                    network.send_to_connection(
                        connection_id,
                        roundo_networking::ServerGameMessage::LocalCoordinateSpawned {
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
                        roundo_networking::ServerGameMessage::LocalCoordinateDespawned {
                            local_coordinate_id,
                        },
                    );
                }
                LocalCoordinateServerEvent::ChunkLoaded {
                    connection_id,
                    local_coordinate_id,
                    chunk,
                } => {
                    network.send_to_connection(
                        connection_id,
                        roundo_networking::ServerGameMessage::LocalCoordinateChunk {
                            local_coordinate_id,
                            coordinate: chunk.coordinate,
                            edge_length: CHUNK_EDGE_LENGTH as u16,
                            voxels: chunk.into_voxels(),
                        },
                    );
                }
                LocalCoordinateServerEvent::ChunkUnloaded {
                    connection_id,
                    local_coordinate_id,
                    coordinate,
                } => {
                    network.send_to_connection(
                        connection_id,
                        roundo_networking::ServerGameMessage::LocalCoordinateChunkUnloaded {
                            local_coordinate_id,
                            coordinate,
                        },
                    );
                }
            }
        }

        if !handled_event {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
