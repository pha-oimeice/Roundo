use log::{debug, info, warn};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateClientCommand, LocalCoordinateClientEvent,
    LocalCoordinateClientIpc,
};
use roundo_marionette::{ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteIpc};
use roundo_networking::{
    CertificatePolicy, ClientGameMessage, ClientHooks, ClientNetwork, ClientNetworkConfig,
    ClientResourceMessage, NetworkError, ServerGameMessage, ServerResourceMessage, StreamId,
    probe_quic_endpoint,
};
use roundo_presence::{ClientPresenceCommand, ClientPresenceIpc};
use roundo_user_config::ServerEntry;
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// The cached connection state exposed at the client-command seam.  This is
/// deliberately independent of the network implementation's callback types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClientConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClientConnectionSnapshot {
    pub status: ClientConnectionStatus,
    pub server: Option<ServerEntry>,
}

/// Cache and asynchronous TCP reachability probes behind one small command
/// seam. A refresh only schedules work; readers never wait for the network.
#[derive(bevy::prelude::Resource, Clone, Default)]
pub struct ServerProbeManager {
    cache: Arc<Mutex<ProbeCache>>,
}

#[derive(Clone, Debug, Default)]
struct ProbeCache {
    revision: u64,
    entries: BTreeMap<usize, ProbeResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeResult {
    pub status: &'static str,
    pub message: Option<String>,
}

impl ServerProbeManager {
    pub fn refresh(&self, servers: &[ServerEntry]) -> u64 {
        let revision = {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.revision += 1;
            let revision = cache.revision;
            cache.entries = servers
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    (
                        index,
                        ProbeResult {
                            status: "probing",
                            message: None,
                        },
                    )
                })
                .collect();
            revision
        };
        for (index, server) in servers.iter().cloned().enumerate() {
            let cache = Arc::clone(&self.cache);
            std::thread::spawn(move || {
                let result = probe_server(&server);
                let mut cache = cache
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                // A newer refresh owns the cache, so stale workers cannot overwrite it.
                if cache.revision == revision {
                    cache.entries.insert(index, result);
                }
            });
        }
        revision
    }

    pub fn result(&self, index: usize) -> ProbeResult {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .get(&index)
            .cloned()
            .unwrap_or(ProbeResult {
                status: "unknown",
                message: None,
            })
    }
}

fn probe_server(server: &ServerEntry) -> ProbeResult {
    let config = match network_config(server) {
        Ok(config) => config,
        Err(error) => {
            return ProbeResult {
                status: "error",
                message: Some(error),
            };
        }
    };
    match probe_quic_endpoint(
        config.quic_address,
        &config.server_name,
        config.certificate_policy,
        Duration::from_secs(2),
    ) {
        Ok(_) => ProbeResult {
            status: "reachable",
            message: None,
        },
        Err(error) => ProbeResult {
            status: "unreachable",
            message: Some(error.to_string()),
        },
    }
}

#[derive(bevy::prelude::Resource)]
pub struct ClientNetworkManager {
    marionette_ipc: ClientMarionetteIpc,
    presence_ipc: ClientPresenceIpc,
    local_coordinate_ipc: LocalCoordinateClientIpc,
    active: Option<ActiveClientConnection>,
    snapshot: ClientConnectionSnapshot,
}

impl ClientNetworkManager {
    pub fn new(
        marionette_ipc: ClientMarionetteIpc,
        presence_ipc: ClientPresenceIpc,
        local_coordinate_ipc: LocalCoordinateClientIpc,
    ) -> Self {
        Self {
            marionette_ipc,
            presence_ipc,
            local_coordinate_ipc,
            active: None,
            snapshot: ClientConnectionSnapshot {
                status: ClientConnectionStatus::Disconnected,
                server: None,
            },
        }
    }
    pub fn connect(&mut self, server: &ServerEntry) -> Result<(), String> {
        let status = self.status().status;
        if self.active.is_some() || status != ClientConnectionStatus::Disconnected {
            return Err(format!(
                "cannot start a connection while client status is {status:?}"
            ));
        }
        info!(
            "Client connection state transition: Disconnected -> Connecting; server={}",
            server.name
        );
        self.snapshot = ClientConnectionSnapshot {
            status: ClientConnectionStatus::Connecting,
            server: Some(server.clone()),
        };
        match ActiveClientConnection::start(
            server,
            self.marionette_ipc.clone(),
            self.presence_ipc.clone(),
            self.local_coordinate_ipc.clone(),
        ) {
            Ok(active) => {
                self.active = Some(active);
                Ok(())
            }
            Err(error) => {
                warn!(
                    "Client connection state transition: Connecting -> Error; server={}; error={error}",
                    server.name
                );
                self.snapshot.status = ClientConnectionStatus::Error {
                    message: error.clone(),
                };
                Err(error)
            }
        }
    }
    pub fn retry(&mut self) -> Result<(), String> {
        let server = self
            .snapshot
            .server
            .clone()
            .ok_or_else(|| "no server is selected".to_string())?;
        // Retry owns termination of the previous attempt. Ordinary connect
        // must never implicitly tear down a session or attempt.
        self.disconnect();
        self.connect(&server)
    }
    pub fn disconnect(&mut self) {
        let previous = self.status().status;
        let server = self
            .snapshot
            .server
            .as_ref()
            .map(|value| value.name.clone());
        info!(
            "Client connection state transition requested: {previous:?} -> Disconnected; server={server:?}"
        );
        if let Some(active) = self.active.take() {
            active.shutdown();
        }
        self.snapshot = ClientConnectionSnapshot {
            status: ClientConnectionStatus::Disconnected,
            server: None,
        };
        info!("Client connection state transition completed: {previous:?} -> Disconnected");
    }
    pub fn status(&self) -> ClientConnectionSnapshot {
        let mut snapshot = self.snapshot.clone();
        if let Some(active) = &self.active {
            snapshot.status = active.status();
        }
        snapshot
    }
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
        presence_ipc: ClientPresenceIpc,
        local_coordinate_ipc: LocalCoordinateClientIpc,
    ) -> Result<Self, String> {
        validate_server_entry(server)?;
        let name = server.name.trim();
        let network_config = network_config(server)?;
        info!(
            "Starting client connection: server={}, quic_address={}",
            name, network_config.quic_address
        );
        let status = Arc::new(RwLock::new(ClientConnectionStatus::Connecting));
        let hooks = Arc::new(ClientHooksAdapter {
            marionette_ipc: marionette_ipc.clone(),
            presence_ipc: presence_ipc.clone(),
            local_coordinate_ipc: local_coordinate_ipc.clone(),
            status: Arc::clone(&status),
        });
        let network = Arc::new(
            ClientNetwork::start(network_config, hooks)
                .map_err(|error| format!("Failed to start network client: {error}"))?,
        );
        let bridge_stop = Arc::new(AtomicBool::new(false));
        let game_network = Arc::clone(&network);
        let game_stop = Arc::clone(&bridge_stop);
        std::thread::spawn(move || bridge_game_ecs_events(marionette_ipc, game_network, game_stop));
        let resource_network = Arc::clone(&network);
        let resource_stop = Arc::clone(&bridge_stop);
        std::thread::spawn(move || {
            bridge_resource_ecs_events(local_coordinate_ipc, resource_network, resource_stop)
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
        debug!("Stopping client connection: server={}", self.name);
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
    let configured = roundo_user_config::load_client_config("roundo-client-config.toml").network;
    let (quic_address, server_name) = resolve_socket_address(
        &server.address,
        configured.endpoint.quic_port,
        "QUIC address",
    )?;
    Ok(ClientNetworkConfig {
        quic_address,
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
    presence_ipc: ClientPresenceIpc,
    local_coordinate_ipc: LocalCoordinateClientIpc,
    status: Arc<RwLock<ClientConnectionStatus>>,
}

impl ClientHooksAdapter {
    fn set_status(&self, status: ClientConnectionStatus) {
        let mut current = self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *current != status {
            info!(
                "Client connection state transition: {:?} -> {status:?}",
                *current
            );
            *current = status;
        }
    }
}

impl ClientHooks for ClientHooksAdapter {
    fn on_server_game_message(&self, message: ServerGameMessage) {
        match message {
            ServerGameMessage::PlayerState { state } => {
                if self
                    .marionette_ipc
                    .try_send(ClientMarionetteCommand::PlayerState(state))
                    .is_err()
                {
                    warn!("Dropped PlayerState message: destination=marionette_ecs");
                }
            }
            ServerGameMessage::PresenceSnapshot { snapshot } => {
                if self
                    .presence_ipc
                    .try_send(ClientPresenceCommand::Snapshot(snapshot))
                    .is_err()
                {
                    warn!("Dropped PresenceSnapshot message: destination=presence_ecs");
                }
            }
        }
    }

    fn on_server_resource_message(&self, message: ServerResourceMessage) {
        match message {
            ServerResourceMessage::LocalCoordinateSpawned {
                local_coordinate_id,
            } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::Spawn(local_coordinate_id))
                    .is_err()
                {
                    warn!(
                        "Dropped LocalCoordinateSpawned message: destination=local_coordinate_ecs, local_coordinate_id={}",
                        local_coordinate_id.0
                    );
                }
            }
            ServerResourceMessage::LocalCoordinateDespawned {
                local_coordinate_id,
            } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::Despawn(local_coordinate_id))
                    .is_err()
                {
                    warn!(
                        "Dropped LocalCoordinateDespawned message: destination=local_coordinate_ecs, local_coordinate_id={}",
                        local_coordinate_id.0
                    );
                }
            }
            ServerResourceMessage::LocalCoordinateChunkVersions { chunks } => {
                let chunk_count = chunks.len();
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::VersionUpdates(chunks))
                    .is_err()
                {
                    warn!(
                        "Dropped LocalCoordinateChunkVersions message: destination=local_coordinate_ecs, chunk_count={chunk_count}"
                    );
                }
            }
            ServerResourceMessage::LocalCoordinateChunk {
                chunk,
                edge_length,
                svo: payload,
            } => {
                if usize::from(edge_length) != CHUNK_EDGE_LENGTH {
                    warn!(
                        "Rejected LocalCoordinateChunk message: local_coordinate_id={}, coordinate={:?}, edge_length={}, expected_edge_length={}",
                        chunk.local_coordinate_id.0,
                        chunk.coordinate,
                        edge_length,
                        CHUNK_EDGE_LENGTH
                    );
                    return;
                }
                let local_coordinate_id = chunk.local_coordinate_id;
                let coordinate = chunk.coordinate;
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::LoadChunk { chunk, payload })
                    .is_err()
                {
                    warn!(
                        "Dropped LocalCoordinateChunk message: destination=local_coordinate_ecs, local_coordinate_id={}, coordinate={coordinate:?}",
                        local_coordinate_id.0
                    );
                }
            }
            ServerResourceMessage::LocalCoordinateChunkUnloaded {
                local_coordinate_id,
                coordinate,
            } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::UnloadChunk {
                        local_coordinate_id,
                        coordinate,
                    })
                    .is_err()
                {
                    warn!(
                        "Dropped LocalCoordinateChunkUnloaded message: destination=local_coordinate_ecs, local_coordinate_id={}, coordinate={coordinate:?}",
                        local_coordinate_id.0
                    );
                }
            }
        }
    }

    fn on_connection_established(&self) {
        info!("Client game connection established");
        self.set_status(ClientConnectionStatus::Connected);
        if self
            .local_coordinate_ipc
            .try_send(LocalCoordinateClientCommand::BeginSession)
            .is_err()
        {
            warn!("Failed to initialize local-coordinate state for connected session");
        }
    }

    fn on_connection_lost(&self) {
        warn!("Client game connection lost; waiting to reconnect");
        self.set_status(ClientConnectionStatus::Reconnecting);
        if self
            .presence_ipc
            .try_send(ClientPresenceCommand::Clear)
            .is_err()
        {
            warn!("Failed to clear presence state after connection loss");
        }
        if self
            .marionette_ipc
            .try_send(ClientMarionetteCommand::ClearPlayerState)
            .is_err()
        {
            warn!("Failed to clear player state after connection loss");
        }
        if self
            .local_coordinate_ipc
            .try_send(LocalCoordinateClientCommand::ResetRequests)
            .is_err()
        {
            warn!("Failed to reset chunk requests after connection loss");
        }
    }

    fn on_connection_error(&self, error: &NetworkError) {
        warn!("Client game connection error: {error}");
        self.set_status(ClientConnectionStatus::Error {
            message: error.to_string(),
        });
    }
}

fn bridge_game_ecs_events(
    marionette_ipc: ClientMarionetteIpc,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started client game IPC bridge");
    while !stop.load(Ordering::Acquire) {
        let mut handled_event = false;
        while let Some(event) = marionette_ipc.try_receive() {
            handled_event = true;
            let message = match event {
                ClientMarionetteEvent::UsePlayerController(command) => {
                    ClientGameMessage::UsePlayerController { command }
                }
            };
            let message_kind = message.kind();
            if let Err(error) = network.send(StreamId::Stream0, message) {
                warn!(
                    "Stopped client network bridge after send failure: message={message_kind}, error={error}"
                );
                return;
            }
        }

        if !handled_event {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    debug!("Stopped client game IPC bridge");
}

fn bridge_resource_ecs_events(
    local_coordinate_ipc: LocalCoordinateClientIpc,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started client resource IPC bridge");
    while !stop.load(Ordering::Acquire) {
        let Some(event) = local_coordinate_ipc.try_receive() else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let message = match event {
            LocalCoordinateClientEvent::RequestChunks(chunks) => {
                ClientResourceMessage::RequestLocalCoordinateChunks { chunks }
            }
        };
        let message_kind = message.kind();
        if let Err(error) = network.send(StreamId::Stream1, message) {
            warn!(
                "Stopped client resource bridge after send failure: message={message_kind}, error={error}"
            );
            return;
        }
    }
    debug!("Stopped client resource IPC bridge");
}

#[cfg(test)]
mod tests {
    use super::{
        ClientConnectionStatus, ClientNetworkManager, ServerProbeManager, network_config,
        resolve_socket_address,
    };
    use roundo_local_coordinate::{LocalCoordinateClientCommand, LocalCoordinateClientEvent};
    use roundo_marionette::{ClientMarionetteCommand, ClientMarionetteEvent};
    use roundo_presence::ClientPresenceCommand;
    use roundo_toolbox::CrossbeamThreadPipe;
    use roundo_user_config::ServerEntry;

    fn network_manager() -> ClientNetworkManager {
        let marionette =
            CrossbeamThreadPipe::<ClientMarionetteCommand, ClientMarionetteEvent>::new();
        let presence = CrossbeamThreadPipe::<ClientPresenceCommand, ()>::new();
        let local_coordinate =
            CrossbeamThreadPipe::<LocalCoordinateClientCommand, LocalCoordinateClientEvent>::new();
        ClientNetworkManager::new(
            marionette.endpoint_a(),
            presence.endpoint_a(),
            local_coordinate.endpoint_a(),
        )
    }

    #[test]
    fn resolves_ip_with_default_port() {
        let (address, server_name) =
            resolve_socket_address("127.0.0.1", 12358, "game address").unwrap();

        assert_eq!(address.to_string(), "127.0.0.1:12358");
        assert_eq!(server_name, "127.0.0.1");
    }

    #[test]
    fn builds_config_from_server_address() {
        let config = network_config(&ServerEntry {
            name: "Test".to_string(),
            address: "127.0.0.1:4000".to_string(),
        })
        .unwrap();

        assert_eq!(config.quic_address.to_string(), "127.0.0.1:4000");
        assert_eq!(config.server_name, "127.0.0.1");
    }

    #[test]
    fn connection_status_is_a_stable_tagged_json_model() {
        assert_eq!(
            serde_json::to_value(ClientConnectionStatus::Error {
                message: "offline".into()
            })
            .unwrap(),
            serde_json::json!({"status":"error","message":"offline"})
        );
        assert_eq!(
            serde_json::to_value(ClientConnectionStatus::Disconnected).unwrap(),
            serde_json::json!({"status":"disconnected"})
        );
    }

    #[test]
    fn connect_rejects_non_idle_state_without_implicitly_disconnecting() {
        let mut manager = network_manager();
        let invalid = ServerEntry {
            name: String::new(),
            address: String::new(),
        };
        assert!(manager.connect(&invalid).is_err());
        assert!(matches!(
            manager.status().status,
            ClientConnectionStatus::Error { .. }
        ));

        let error = manager.connect(&invalid).unwrap_err();
        assert!(error.contains("cannot start a connection"));
        assert!(matches!(
            manager.status().status,
            ClientConnectionStatus::Error { .. }
        ));
    }

    #[test]
    fn refresh_is_nonblocking_and_advances_the_probe_revision() {
        let probes = ServerProbeManager::default();
        assert_eq!(probes.refresh(&[]), 1);
        assert_eq!(probes.refresh(&[]), 2);
    }

    #[test]
    fn an_unprobed_server_has_a_stable_unknown_cache_entry() {
        let probes = ServerProbeManager::default();
        let result = probes.result(42);
        assert_eq!(result.status, "unknown");
        assert_eq!(result.message, None);
    }

    #[test]
    fn resolves_hostnames_without_losing_tls_server_name() {
        let (address, server_name) =
            resolve_socket_address("localhost:4000", 12358, "game address").unwrap();

        assert_eq!(address.port(), 4000);
        assert_eq!(server_name, "localhost");
    }
}
