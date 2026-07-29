use log::{debug, warn};
use roundo_marionette::{ServerMarionetteCommand, ServerMarionetteEvent, ServerMarionetteIpc};
use roundo_networking::{
    ClientGameMessage, ConnectionId, HookFuture, PublicSession, ServerHooks, ServerNetwork,
    ServerNetworkConfig, SessionId, UserSession,
};
use std::path::PathBuf;
use std::sync::Arc;

pub fn start_server(marionette_ipc: ServerMarionetteIpc) {
    let config = network_config();
    debug!(
        "Starting network server: game={}, public={}, certificate_directory={}",
        config.game_address,
        config.public_address,
        config.certificate_directory.display()
    );
    let hooks = Arc::new(ServerHooksAdapter {
        marionette_ipc: marionette_ipc.clone(),
    });
    let network = ServerNetwork::start(config, hooks)
        .unwrap_or_else(|error| panic!("failed to start network server: {error}"));
    let addresses = network.addresses();
    debug!(
        "Network server is ready: game={}, public={}",
        addresses.game_address, addresses.public_address
    );

    std::thread::spawn(move || bridge_marionette_events(marionette_ipc, network));
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
    }

    fn on_client_game_message(
        &self,
        connection_id: ConnectionId,
        user_session: UserSession,
        message: ClientGameMessage,
    ) {
        let command = match message {
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

fn bridge_marionette_events(marionette_ipc: ServerMarionetteIpc, network: ServerNetwork) {
    debug!("Started Marionette-to-network event bridge");
    while let Some(event) = marionette_ipc.receive() {
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
    debug!("Marionette-to-network event bridge stopped");
}
