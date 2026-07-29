use roundo_marionette::{ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteIpc};
use roundo_networking::{
    CertificatePolicy, ClientGameMessage, ClientHooks, ClientNetwork, ClientNetworkConfig,
    ServerGameMessage,
};
use std::sync::Arc;
use std::time::Duration;

pub fn start_client(marionette_ipc: ClientMarionetteIpc) {
    let hooks = Arc::new(ClientHooksAdapter {
        marionette_ipc: marionette_ipc.clone(),
    });
    let network = ClientNetwork::start(network_config(), hooks)
        .unwrap_or_else(|error| panic!("failed to start network client: {error}"));

    std::thread::spawn(move || bridge_marionette_events(marionette_ipc, network));
}

fn network_config() -> ClientNetworkConfig {
    let network = &crate::config::CLIENT_CONFIG.network;
    ClientNetworkConfig {
        game_address: network.endpoint.get_game_addr(),
        public_address: network.endpoint.get_https_addr(),
        server_name: network.endpoint.host.clone(),
        certificate_policy: if network.ca_verification {
            CertificatePolicy::SystemRoots
        } else {
            CertificatePolicy::TrustOnFirstUse
        },
        reconnect_delay: Duration::from_secs(5),
    }
}

struct ClientHooksAdapter {
    marionette_ipc: ClientMarionetteIpc,
}

impl ClientHooks for ClientHooksAdapter {
    fn on_server_game_message(&self, message: ServerGameMessage) {
        let command = match message {
            ServerGameMessage::ControllerGranted { controller } => {
                ClientMarionetteCommand::ControllerGranted { controller }
            }
            ServerGameMessage::ControllerRevoked { controller_id } => {
                ClientMarionetteCommand::ControllerRevoked { controller_id }
            }
            ServerGameMessage::ViewCameraState {
                controller_id,
                state,
            } => ClientMarionetteCommand::ViewCameraState {
                controller_id,
                state,
            },
        };
        let _ = self.marionette_ipc.try_send(command);
    }
}

fn bridge_marionette_events(marionette_ipc: ClientMarionetteIpc, network: ClientNetwork) {
    while let Some(event) = marionette_ipc.receive() {
        let message = match event {
            ClientMarionetteEvent::RequestController { controller_id } => {
                ClientGameMessage::RequestController { controller_id }
            }
            ClientMarionetteEvent::ReleaseController { controller_id } => {
                ClientGameMessage::ReleaseController { controller_id }
            }
            ClientMarionetteEvent::ControllerInput {
                controller_id,
                input,
            } => ClientGameMessage::ControllerInput {
                controller_id,
                input,
            },
        };
        if network.send(message).is_err() {
            return;
        }
    }
}
