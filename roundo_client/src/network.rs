use crate::config::ServerEntry;
use roundo_character::{ClientCharacterCommand, ClientCharacterEvent, ClientCharacterIpc};
use roundo_marionette::{ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteIpc};
use roundo_networking::{
    CertificatePolicy, ClientGameMessage, ClientHooks, ClientNetwork, ClientNetworkConfig,
    NetworkError, ServerGameMessage,
};
use static_voxel::{GeneratedChunk, StaticVoxelClientCommand, StaticVoxelClientIpc};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientConnectionStatus {
    Connecting,
    Connected,
    Reconnecting,
    ConnectionError(String),
}

pub struct ActiveClientConnection {
    name: String,
    network: Arc<ClientNetwork>,
    bridge_stop: Arc<AtomicBool>,
    status: Arc<RwLock<ClientConnectionStatus>>,
}

impl ActiveClientConnection {
    pub fn start(
        server: &ServerEntry,
        marionette_ipc: ClientMarionetteIpc,
        character_ipc: ClientCharacterIpc,
        static_voxel_ipc: StaticVoxelClientIpc,
    ) -> Result<Self, String> {
        validate_server_entry(server)?;
        let name = server.name.trim();
        let status = Arc::new(RwLock::new(ClientConnectionStatus::Connecting));
        let hooks = Arc::new(ClientHooksAdapter {
            marionette_ipc: marionette_ipc.clone(),
            character_ipc: character_ipc.clone(),
            static_voxel_ipc,
            status: Arc::clone(&status),
        });
        let network = Arc::new(
            ClientNetwork::start(network_config(server)?, hooks)
                .map_err(|error| format!("Failed to start network client: {error}"))?,
        );
        let bridge_stop = Arc::new(AtomicBool::new(false));
        let bridge_network = Arc::clone(&network);
        let thread_stop = Arc::clone(&bridge_stop);
        std::thread::spawn(move || {
            bridge_ecs_events(marionette_ipc, character_ipc, bridge_network, thread_stop)
        });

        Ok(Self {
            name: name.to_string(),
            network,
            bridge_stop,
            status,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn status(&self) -> ClientConnectionStatus {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn shutdown(&self) {
        self.bridge_stop.store(true, Ordering::Release);
        self.network.shutdown();
    }
}

impl Drop for ActiveClientConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn validate_server_entry(server: &ServerEntry) -> Result<(), String> {
    if server.name.trim().is_empty() {
        return Err("Server name cannot be empty".to_string());
    }
    network_config(server).map(|_| ())
}

fn network_config(server: &ServerEntry) -> Result<ClientNetworkConfig, String> {
    let configured = &crate::config::CLIENT_CONFIG.network;
    let (game_address, server_name) = resolve_socket_address(
        &server.game_addr,
        configured.endpoint.game_port,
        "game address",
    )?;
    let (public_address, _) = resolve_socket_address(
        &server.https_addr,
        configured.endpoint.https_port,
        "HTTPS address",
    )?;
    Ok(ClientNetworkConfig {
        game_address,
        public_address,
        server_name,
        certificate_policy: if configured.ca_verification {
            CertificatePolicy::SystemRoots
        } else {
            CertificatePolicy::TrustOnFirstUse
        },
        reconnect_delay: Duration::from_secs(5),
    })
}

pub(crate) fn resolve_socket_address(
    address: &str,
    default_port: u16,
    field_name: &str,
) -> Result<(SocketAddr, String), String> {
    let address = address.trim();
    if address.is_empty() {
        return Err(format!("{field_name} cannot be empty"));
    }
    if let Ok(socket_address) = address.parse::<SocketAddr>() {
        return Ok((socket_address, socket_address.ip().to_string()));
    }
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Ok((SocketAddr::new(ip, default_port), ip.to_string()));
    }

    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| format!("Invalid {field_name} port"))?,
        ),
        None => (address, default_port),
    };
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let resolved = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("Failed to resolve {field_name}: {error}"))?
        .next()
        .ok_or_else(|| format!("{field_name} did not resolve to an address"))?;
    Ok((resolved, host.to_string()))
}

struct ClientHooksAdapter {
    marionette_ipc: ClientMarionetteIpc,
    character_ipc: ClientCharacterIpc,
    static_voxel_ipc: StaticVoxelClientIpc,
    status: Arc<RwLock<ClientConnectionStatus>>,
}

impl ClientHooksAdapter {
    fn set_status(&self, status: ClientConnectionStatus) {
        *self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
    }
}

impl ClientHooks for ClientHooksAdapter {
    fn on_server_game_message(&self, message: ServerGameMessage) {
        match message {
            ServerGameMessage::CharacterSnapshot { snapshot } => {
                let _ = self
                    .character_ipc
                    .try_send(ClientCharacterCommand::Snapshot(snapshot));
            }
            ServerGameMessage::CharacterControlGranted { character_id } => {
                let _ = self
                    .character_ipc
                    .try_send(ClientCharacterCommand::ControlGranted { character_id });
            }
            ServerGameMessage::StaticVoxelChunk {
                coordinate,
                edge_length,
                voxels,
            } => {
                if usize::from(edge_length) != static_voxel::CHUNK_EDGE_LENGTH {
                    return;
                }
                if let Some(chunk) = GeneratedChunk::from_voxels(coordinate, voxels) {
                    let _ = self
                        .static_voxel_ipc
                        .try_send(StaticVoxelClientCommand::LoadChunk(chunk));
                }
            }
            ServerGameMessage::StaticVoxelChunkUnloaded { coordinate } => {
                let _ = self
                    .static_voxel_ipc
                    .try_send(StaticVoxelClientCommand::UnloadChunk(coordinate));
            }
            ServerGameMessage::ControllerGranted { controller } => {
                let _ = self
                    .marionette_ipc
                    .try_send(ClientMarionetteCommand::ControllerGranted { controller });
            }
            ServerGameMessage::ControllerRevoked { controller_id } => {
                let _ = self
                    .marionette_ipc
                    .try_send(ClientMarionetteCommand::ControllerRevoked { controller_id });
            }
            ServerGameMessage::ViewCameraState {
                controller_id,
                state,
            } => {
                let _ = self
                    .marionette_ipc
                    .try_send(ClientMarionetteCommand::ViewCameraState {
                        controller_id,
                        state,
                    });
            }
        }
    }

    fn on_connection_established(&self) {
        self.set_status(ClientConnectionStatus::Connected);
    }

    fn on_connection_lost(&self) {
        self.set_status(ClientConnectionStatus::Reconnecting);
    }

    fn on_connection_error(&self, error: &NetworkError) {
        self.set_status(ClientConnectionStatus::ConnectionError(error.to_string()));
    }
}

fn bridge_ecs_events(
    marionette_ipc: ClientMarionetteIpc,
    character_ipc: ClientCharacterIpc,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        let mut handled_event = false;
        while let Some(event) = marionette_ipc.try_receive() {
            handled_event = true;
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

        while let Some(event) = character_ipc.try_receive() {
            handled_event = true;
            let ClientCharacterEvent::RequestControl { character_id } = event;
            if network
                .send(ClientGameMessage::RequestCharacterControl { character_id })
                .is_err()
            {
                return;
            }
        }

        if !handled_event {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{network_config, resolve_socket_address};
    use crate::config::ServerEntry;

    #[test]
    fn resolves_ip_with_default_port() {
        let (address, server_name) =
            resolve_socket_address("127.0.0.1", 12358, "game address").unwrap();

        assert_eq!(address.to_string(), "127.0.0.1:12358");
        assert_eq!(server_name, "127.0.0.1");
    }

    #[test]
    fn builds_config_from_separate_game_and_https_addresses() {
        let config = network_config(&ServerEntry {
            name: "Test".to_string(),
            game_addr: "127.0.0.1:4000".to_string(),
            https_addr: "127.0.0.1:5000".to_string(),
        })
        .unwrap();

        assert_eq!(config.game_address.to_string(), "127.0.0.1:4000");
        assert_eq!(config.public_address.to_string(), "127.0.0.1:5000");
        assert_eq!(config.server_name, "127.0.0.1");
    }

    #[test]
    fn resolves_hostnames_without_losing_tls_server_name() {
        let (address, server_name) =
            resolve_socket_address("localhost:4000", 12358, "game address").unwrap();

        assert_eq!(address.port(), 4000);
        assert_eq!(server_name, "localhost");
    }
}
