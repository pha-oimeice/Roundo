//! Client composition for connection state, endpoint probes, and ECS/network bridges.
//!
//! Connection callbacks update a thread-safe status snapshot and submit domain
//! commands without blocking. Separate polling threads forward ECS controller and
//! chunk-demand events into bounded networking queues.

use log::{debug, info, warn};
use roundo_local_coordinate::{
    CHUNK_EDGE_LENGTH, LocalCoordinateClientCommand, LocalCoordinateClientEvent,
    LocalCoordinateClientIpc,
};
use roundo_marionette::{ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteIpc};
use roundo_networking::{
    CertificatePolicy, ClientGameMessage, ClientHooks, ClientNetwork,
    ClientNetworkConfig as NetworkRuntimeConfig, ClientResourceMessage, NetworkError,
    ResourceCatalogFingerprint, ServerGameMessage, ServerResourceMessage, StreamId,
    probe_quic_endpoint,
};
use roundo_presence::{ClientPresenceCommand, ClientPresenceIpc};
use roundo_toolbox::{BridgeStep, BridgeThreadGroup, run_polling_bridge};
use roundo_user_config::{ClientNetworkConfig as ClientNetworkSettings, ServerEntry};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::AtomicBool;
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

/// Owned point-in-time connection state returned to command/UI callers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClientConnectionSnapshot {
    pub status: ClientConnectionStatus,
    /// Selected server while connecting, connected, reconnecting, or in error.
    pub server: Option<ServerEntry>,
}

/// Cache and asynchronous QUIC endpoint probes behind one small command seam.
/// A refresh only schedules work; readers never wait for the network.
#[derive(bevy::prelude::Resource, Clone, Default)]
pub struct ServerProbeManager {
    cache: Arc<Mutex<ProbeCache>>,
}

#[derive(Clone, Debug, Default)]
struct ProbeCache {
    /// Identifies the server-list snapshot owned by the current probe batch.
    revision: u64,
    /// Advances for every externally visible cache change.
    change_revision: u64,
    entries: BTreeMap<usize, ProbeCacheEntry>,
}

#[derive(Clone, Debug)]
struct ProbeCacheEntry {
    server: ServerEntry,
    result: ProbeResult,
}

/// Owned endpoint-probe result suitable for direct JSON serialization.
#[derive(Clone, Debug, Serialize)]
pub struct ProbeResult {
    /// One of `probing`, `reachable`, `unreachable`, `error`, or `unknown`.
    pub status: &'static str,
    /// Diagnostic detail for `unreachable` and local configuration `error`.
    pub message: Option<String>,
}

impl ServerProbeManager {
    /// Replaces the cache with `probing` entries and spawns one probe thread per server.
    ///
    /// Returns the new batch revision without waiting for DNS, TLS, or QUIC I/O.
    /// Results from older batches are discarded. Probe threads are detached and
    /// cannot be cancelled through this manager.
    pub fn refresh(
        &self,
        servers: &[ServerEntry],
        network_settings: &ClientNetworkSettings,
    ) -> u64 {
        let revision = {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.revision += 1;
            cache.change_revision += 1;
            let revision = cache.revision;
            cache.entries = servers
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, server)| {
                    (
                        index,
                        ProbeCacheEntry {
                            server,
                            result: ProbeResult {
                                status: "probing",
                                message: None,
                            },
                        },
                    )
                })
                .collect();
            revision
        };
        for (index, server) in servers.iter().cloned().enumerate() {
            let cache = Arc::clone(&self.cache);
            let network_settings = network_settings.clone();
            std::thread::spawn(move || {
                let result = probe_server(&server, &network_settings);
                let mut cache = cache
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                // A newer refresh or any server-list mutation owns the cache,
                // so stale workers cannot attach a result to another endpoint.
                let cached_entry = cache.entries.get(&index);
                let owns_entry = cache.revision == revision
                    && cached_entry.is_some_and(|entry| entry.server == server);
                if owns_entry {
                    if let Some(entry) = cache.entries.get_mut(&index) {
                        entry.result = result;
                    }
                    cache.change_revision += 1;
                }
            });
        }
        revision
    }

    /// Returns a cloned cached result only when both index and server value match.
    ///
    /// Missing or stale entries return `unknown`; this method does not start a probe.
    pub fn result(&self, index: usize, server: &ServerEntry) -> ProbeResult {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = cache.entries.get(&index);
        entry
            .filter(|entry| &entry.server == server)
            .map(|entry| entry.result.clone())
            .unwrap_or(ProbeResult {
                status: "unknown",
                message: None,
            })
    }

    /// Invalidates the complete batch when the indexed server list changes.
    /// This also prevents in-flight workers from publishing against shifted
    /// indices.
    pub fn invalidate(&self) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.revision += 1;
        cache.change_revision += 1;
        cache.entries.clear();
    }

    /// Returns a cache-change counter for polling UI invalidation.
    ///
    /// Refresh, invalidation, and each accepted worker result advance the counter.
    pub fn change_revision(&self) -> u64 {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .change_revision
    }
}

fn probe_server(server: &ServerEntry, network_settings: &ClientNetworkSettings) -> ProbeResult {
    let config = match network_config(server, network_settings) {
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

/// Bevy-owned authority for one selected server and at most one active attempt.
#[derive(bevy::prelude::Resource)]
pub struct ClientNetworkManager {
    network_settings: ClientNetworkSettings,
    marionette_ipc: ClientMarionetteIpc,
    presence_ipc: ClientPresenceIpc,
    local_coordinate_ipc: LocalCoordinateClientIpc,
    resource_fingerprint: ResourceCatalogFingerprint,
    active: Option<ActiveClientConnection>,
    snapshot: ClientConnectionSnapshot,
}

impl ClientNetworkManager {
    /// Creates a disconnected manager over the supplied domain IPC endpoints.
    pub fn new(
        network_settings: ClientNetworkSettings,
        marionette_ipc: ClientMarionetteIpc,
        presence_ipc: ClientPresenceIpc,
        local_coordinate_ipc: LocalCoordinateClientIpc,
        resource_fingerprint: ResourceCatalogFingerprint,
    ) -> Self {
        Self {
            network_settings,
            marionette_ipc,
            presence_ipc,
            local_coordinate_ipc,
            resource_fingerprint,
            active: None,
            snapshot: ClientConnectionSnapshot {
                status: ClientConnectionStatus::Disconnected,
                server: None,
            },
        }
    }
    /// Starts one asynchronous reconnecting attempt from the disconnected state.
    ///
    /// Local validation/runtime startup failure leaves the selected server and an
    /// `Error` status for retry. Later dial failures are reflected asynchronously
    /// through [`Self::status`].
    ///
    /// # Errors
    ///
    /// Returns an error if another attempt/session is active or local connection
    /// setup fails.
    pub fn start_connection(&mut self, server: &ServerEntry) -> Result<(), String> {
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
            &self.network_settings,
            self.marionette_ipc.clone(),
            self.presence_ipc.clone(),
            self.local_coordinate_ipc.clone(),
            self.resource_fingerprint,
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
    /// Stops the current attempt, then starts a fresh attempt to the selected server.
    ///
    /// # Errors
    ///
    /// Returns an error if no server is selected or fresh startup fails.
    pub fn retry(&mut self) -> Result<(), String> {
        let server = self
            .snapshot
            .server
            .clone()
            .ok_or_else(|| "no server is selected".to_string())?;
        // Retry owns termination of the previous attempt. Ordinary connect
        // must never implicitly tear down a session or attempt.
        self.disconnect();
        self.start_connection(&server)
    }
    /// Stops and joins the active connection runtime and clears server selection.
    ///
    /// This blocks the current OS thread. It does not itself enqueue domain-state
    /// clear commands; connection-loss callbacks own transient-loss cleanup.
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
        if let Some(mut active) = self.active.take() {
            active.shutdown();
        }
        self.snapshot = ClientConnectionSnapshot {
            status: ClientConnectionStatus::Disconnected,
            server: None,
        };
        info!("Client connection state transition completed: {previous:?} -> Disconnected");
    }
    /// Returns an owned snapshot, overlaying live callback status when active.
    pub fn status(&self) -> ClientConnectionSnapshot {
        let mut snapshot = self.snapshot.clone();
        if let Some(active) = &self.active {
            snapshot.status = active.status();
        }
        snapshot
    }
}

/// Owns one reconnecting network runtime and its two ECS outbound bridges.
pub struct ActiveClientConnection {
    name: String,
    network: Arc<ClientNetwork>,
    bridges: BridgeThreadGroup,
    status: Arc<RwLock<ClientConnectionStatus>>,
}

impl ActiveClientConnection {
    /// Validates the server, starts networking, and spawns game/resource bridges.
    ///
    /// Success does not mean a remote session is established; status begins as
    /// `Connecting`. If bridge creation fails, already-created owners are dropped
    /// during error unwinding and shut down their workers.
    pub fn start(
        server: &ServerEntry,
        network_settings: &ClientNetworkSettings,
        marionette_ipc: ClientMarionetteIpc,
        presence_ipc: ClientPresenceIpc,
        local_coordinate_ipc: LocalCoordinateClientIpc,
        resource_fingerprint: ResourceCatalogFingerprint,
    ) -> Result<Self, String> {
        validate_server_entry(server, network_settings)?;
        let name = server.name.trim();
        let network_config = network_config(server, network_settings)?;
        info!(
            "Starting client connection: server={}, quic_address={}",
            name, network_config.quic_address
        );
        let status = Arc::new(RwLock::new(ClientConnectionStatus::Connecting));
        let hooks = Arc::new(ClientHooksAdapter {
            marionette_ipc: marionette_ipc.clone(),
            presence_ipc: presence_ipc.clone(),
            local_coordinate_ipc: local_coordinate_ipc.clone(),
            resource_fingerprint,
            status: Arc::clone(&status),
        });
        let network = Arc::new(
            ClientNetwork::start(network_config, hooks)
                .map_err(|error| format!("Failed to start network client: {error}"))?,
        );
        let mut bridges = BridgeThreadGroup::new();
        let game_network = Arc::clone(&network);
        bridges
            .spawn("roundo-client-game-bridge", move |stop| {
                bridge_game_ecs_events(marionette_ipc, game_network, stop)
            })
            .map_err(|error| format!("failed to start client game bridge: {error}"))?;
        let resource_network = Arc::clone(&network);
        bridges
            .spawn("roundo-client-resource-bridge", move |stop| {
                bridge_resource_ecs_events(local_coordinate_ipc, resource_network, stop)
            })
            .map_err(|error| format!("failed to start client resource bridge: {error}"))?;

        Ok(Self {
            name: name.to_string(),
            network,
            bridges,
            status,
        })
    }

    /// Returns the trimmed server display name captured at startup.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a cloned momentary status updated by networking callbacks.
    pub fn status(&self) -> ClientConnectionStatus {
        let status = self.status.read();
        status
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Signals and joins both ECS bridges before stopping the network runtime.
    ///
    /// This blocks the current OS thread and is idempotent after worker handles
    /// are drained.
    pub fn shutdown(&mut self) {
        debug!("Stopping client connection: server={}", self.name);
        self.bridges.shutdown();
        self.network.shutdown();
    }
}

impl Drop for ActiveClientConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Validates a nonempty display name and resolves the configured endpoint.
///
/// Resolution is synchronous and may perform DNS. Success is only local
/// configuration validation; no QUIC connection is attempted.
pub fn validate_server_entry(
    server: &ServerEntry,
    configured: &ClientNetworkSettings,
) -> Result<(), String> {
    if server.name.trim().is_empty() {
        return Err("Server name cannot be empty".to_string());
    }
    network_config(server, configured).map(|_| ())
}

fn network_config(
    server: &ServerEntry,
    configured: &ClientNetworkSettings,
) -> Result<NetworkRuntimeConfig, String> {
    let (quic_address, server_name) = resolve_socket_address(
        &server.address,
        configured.endpoint.quic_port,
        "QUIC address",
    )?;
    Ok(NetworkRuntimeConfig {
        quic_address,
        server_name,
        certificate_policy: if configured.ca_verification {
            CertificatePolicy::SystemRoots
        } else {
            CertificatePolicy::TrustOnFirstUse
        },
        reconnect_delay: Duration::from_secs(5),
        admission: roundo_networking::TransportAdmissionPolicy::default(),
    })
}

/// Resolves endpoint text and preserves the host text needed for TLS SNI.
///
/// Explicit socket addresses retain their numeric IP as server name; bare IPs
/// receive `default_port`; hostnames may include an explicit port and resolve
/// synchronously. When DNS returns multiple addresses, only the first is kept.
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

/// Projects network-runtime callbacks into domain queues and connection status.
struct ClientHooksAdapter {
    marionette_ipc: ClientMarionetteIpc,
    presence_ipc: ClientPresenceIpc,
    local_coordinate_ipc: LocalCoordinateClientIpc,
    resource_fingerprint: ResourceCatalogFingerprint,
    status: Arc<RwLock<ClientConnectionStatus>>,
}

impl ClientHooksAdapter {
    fn set_status(&self, status: ClientConnectionStatus) {
        let current = self.status.write();
        let mut current = current.unwrap_or_else(|poisoned| poisoned.into_inner());
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
    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
    }

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
            ServerResourceMessage::RenderingAnchorSpawned { anchor } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::RenderingAnchorSpawned(anchor))
                    .is_err()
                {
                    warn!(
                        "Dropped RenderingAnchorSpawned message: destination=local_coordinate_ecs"
                    );
                }
            }
            ServerResourceMessage::RenderingAnchorUpdated { anchor } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::RenderingAnchorUpdated(anchor))
                    .is_err()
                {
                    warn!(
                        "Dropped RenderingAnchorUpdated message: destination=local_coordinate_ecs"
                    );
                }
            }
            ServerResourceMessage::RenderingAnchorDespawned { anchor_id } => {
                if self
                    .local_coordinate_ipc
                    .try_send(LocalCoordinateClientCommand::RenderingAnchorDespawned(
                        anchor_id,
                    ))
                    .is_err()
                {
                    warn!(
                        "Dropped RenderingAnchorDespawned message: destination=local_coordinate_ecs"
                    );
                }
            }
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

/// Forwards sequenced controller events to stream 0 until shutdown or send failure.
///
/// The event that encounters saturation/closure is dropped, and the bridge stops
/// permanently; it does not retry after network reconnection.
fn bridge_game_ecs_events(
    marionette_ipc: ClientMarionetteIpc,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started client game IPC bridge");
    run_polling_bridge(&stop, || {
        let Some(event) = marionette_ipc.try_receive() else {
            return BridgeStep::Idle;
        };
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
            return BridgeStep::Stop;
        }
        BridgeStep::Forwarded
    });
    debug!("Stopped client game IPC bridge");
}

/// Forwards chunk demand to stream 1 until shutdown or send failure.
///
/// The event that encounters saturation/closure is dropped, and the bridge stops
/// permanently; it does not retry after network reconnection.
fn bridge_resource_ecs_events(
    local_coordinate_ipc: LocalCoordinateClientIpc,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started client resource IPC bridge");
    run_polling_bridge(&stop, || {
        let Some(event) = local_coordinate_ipc.try_receive() else {
            return BridgeStep::Idle;
        };
        let message = match event {
            LocalCoordinateClientEvent::RequestChunks(chunks) => {
                ClientResourceMessage::RequestLocalCoordinateChunks { chunks }
            }
            LocalCoordinateClientEvent::SetChunkViewDistance { chunks } => {
                ClientResourceMessage::SetChunkViewDistance { chunks }
            }
        };
        let message_kind = message.kind();
        if let Err(error) = network.send(StreamId::Stream1, message) {
            warn!(
                "Stopped client resource bridge after send failure: message={message_kind}, error={error}"
            );
            return BridgeStep::Stop;
        }
        BridgeStep::Forwarded
    });
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
    use std::{
        net::UdpSocket,
        time::{Duration, Instant},
    };

    fn network_manager() -> ClientNetworkManager {
        let marionette =
            CrossbeamThreadPipe::<ClientMarionetteCommand, ClientMarionetteEvent>::new();
        let presence = CrossbeamThreadPipe::<ClientPresenceCommand, ()>::new();
        let local_coordinate =
            CrossbeamThreadPipe::<LocalCoordinateClientCommand, LocalCoordinateClientEvent>::new();
        ClientNetworkManager::new(
            roundo_user_config::ClientNetworkConfig::default(),
            marionette.endpoint_a(),
            presence.endpoint_a(),
            local_coordinate.endpoint_a(),
            roundo_networking::ResourceCatalogFingerprint::default(),
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
    fn builds_config_from_the_runtime_owned_network_settings() {
        let mut settings = roundo_user_config::ClientNetworkConfig::default();
        settings.endpoint.quic_port = 4000;
        settings.ca_verification = true;
        let config = network_config(
            &ServerEntry {
                name: "Test".to_string(),
                address: "127.0.0.1".to_string(),
            },
            &settings,
        )
        .unwrap();

        assert_eq!(config.quic_address.to_string(), "127.0.0.1:4000");
        assert_eq!(config.server_name, "127.0.0.1");
        assert_eq!(
            config.certificate_policy,
            roundo_networking::CertificatePolicy::SystemRoots
        );
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
        assert!(manager.start_connection(&invalid).is_err());
        assert!(matches!(
            manager.status().status,
            ClientConnectionStatus::Error { .. }
        ));

        let error = manager.start_connection(&invalid).unwrap_err();
        assert!(error.contains("cannot start a connection"));
        assert!(matches!(
            manager.status().status,
            ClientConnectionStatus::Error { .. }
        ));
    }

    #[test]
    fn disconnect_cancels_an_in_progress_connection_promptly() {
        let blackhole = UdpSocket::bind("127.0.0.1:0").unwrap();
        blackhole
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut manager = network_manager();
        manager
            .start_connection(&ServerEntry {
                name: "blackhole".into(),
                address: blackhole.local_addr().unwrap().to_string(),
            })
            .unwrap();

        let mut packet = [0; 2048];
        blackhole.recv_from(&mut packet).unwrap();
        let started = Instant::now();
        manager.disconnect();

        assert!(
            started.elapsed() < Duration::from_millis(500),
            "disconnect took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn refresh_is_nonblocking_and_advances_the_probe_revision() {
        let probes = ServerProbeManager::default();
        let settings = roundo_user_config::ClientNetworkConfig::default();
        assert_eq!(probes.refresh(&[], &settings), 1);
        assert_eq!(probes.refresh(&[], &settings), 2);
    }

    #[test]
    fn an_unprobed_server_has_a_stable_unknown_cache_entry() {
        let probes = ServerProbeManager::default();
        let result = probes.result(
            42,
            &ServerEntry {
                name: "missing".into(),
                address: "127.0.0.1:1".into(),
            },
        );
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
