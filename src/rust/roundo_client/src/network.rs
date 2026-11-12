//! Client composition for connection state and ECS/network bridges.
//!
//! Connection callbacks update a thread-safe status snapshot and submit domain
//! commands without blocking. Separate polling threads forward ECS controller and
//! chunk-demand events into bounded networking queues.

use log::{debug, info, warn};
use roundo_contracts::{
    ClientResourceMessage, ControllerId, ResourceCatalogFingerprint,
    control_handoff::ControlRequest,
};
use roundo_game_network_bridge::{
    ClientConnectionEvent, ClientGameSessionLifecycle, ClientHooksAdapter, bridge_game_ecs_events,
};
use roundo_game_network_bridge::{ControlAcquireStatus, ControlRetryError};
use roundo_local_coordinate::{LocalCoordinateClientEvent, LocalCoordinateClientIpc};
use roundo_marionette::ClientMarionetteIpc;
use roundo_networking::{
    CertificatePolicy, ClientNetwork, ClientNetworkConfig as NetworkRuntimeConfig,
};
use roundo_presence::ClientPresenceIpc;
use roundo_toolbox::{BridgeStep, BridgeThreadGroup, run_polling_bridge};
use roundo_user_config::{ClientNetworkConfig as ClientNetworkSettings, ServerEntry};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
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

/// Bevy-owned authority for one selected server and at most one active attempt.
#[derive(bevy::prelude::Resource)]
pub struct ClientNetworkManager {
    network_settings: ClientNetworkSettings,
    marionette_ipc: ClientMarionetteIpc,
    local_coordinate_ipc: LocalCoordinateClientIpc,
    session_lifecycle: ClientGameSessionLifecycle,
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
        let session_lifecycle = ClientGameSessionLifecycle::new(
            marionette_ipc.clone(),
            presence_ipc.clone(),
            local_coordinate_ipc.clone(),
        );
        Self {
            network_settings,
            marionette_ipc,
            local_coordinate_ipc,
            session_lifecycle,
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
            self.local_coordinate_ipc.clone(),
            self.session_lifecycle.clone(),
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
    /// This blocks the current OS thread. After all transport callbacks stop, it
    /// ends the same authority session used by transient connection loss.
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
        self.session_lifecycle.end();
        self.snapshot = ClientConnectionSnapshot {
            status: ClientConnectionStatus::Disconnected,
            server: None,
        };
        info!("Client connection state transition completed: {previous:?} -> Disconnected");
    }
    /// Returns every current-epoch Acquire projection in stable controller order.
    /// The values are lifecycle/UI state only, not transport bindings or ECS entities.
    pub fn control_acquire_statuses(&self) -> Vec<ControlAcquireStatus> {
        self.session_lifecycle.control_acquire_statuses()
    }

    /// Retries only an Acquire failure delivered in the current active epoch.
    /// Unlike [`Self::retry`], this neither disconnects nor creates another session.
    pub fn retry_failed_acquire(
        &self,
        controller: ControllerId,
    ) -> Result<ControlRequest, ControlRetryError> {
        let active = self
            .active
            .as_ref()
            .ok_or(ControlRetryError::StaleSession)?;
        self.session_lifecycle
            .retry_failed_acquire(&active.network, controller)
    }

    pub fn submit_debug(
        &self,
        operation: roundo_contracts::debug::DebugOperation,
    ) -> Result<String, String> {
        let active = self.active.as_ref().ok_or("尚未连接到服务端")?;
        self.session_lifecycle
            .send_debug(&active.network, operation)
    }

    pub fn inventory_view(
        &self,
        subject: Option<(u64, ControllerId, roundo_contracts::ControllerBindingId)>,
    ) -> roundo_game_network_bridge::InventoryView {
        self.session_lifecycle.inventory_view(
            self.active.as_ref().map(|active| active.network.as_ref()),
            subject,
        )
    }

    pub fn debug_status(&self) -> roundo_game_network_bridge::DebugStatusSnapshot {
        self.session_lifecycle.debug_status()
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
struct ActiveClientConnection {
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
    fn start(
        server: &ServerEntry,
        network_settings: &ClientNetworkSettings,
        marionette_ipc: ClientMarionetteIpc,
        local_coordinate_ipc: LocalCoordinateClientIpc,
        session_lifecycle: ClientGameSessionLifecycle,
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
        let callback_status = Arc::clone(&status);
        let hooks = Arc::new(ClientHooksAdapter::new(
            local_coordinate_ipc.clone(),
            session_lifecycle.clone(),
            resource_fingerprint,
            move |event| {
                let next = match event {
                    ClientConnectionEvent::Established => ClientConnectionStatus::Connected,
                    ClientConnectionEvent::Lost => ClientConnectionStatus::Reconnecting,
                    ClientConnectionEvent::Error(message) => {
                        ClientConnectionStatus::Error { message }
                    }
                };
                let mut current = callback_status
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if *current != next {
                    info!(
                        "Client connection state transition: {:?} -> {next:?}",
                        *current
                    );
                    *current = next;
                }
            },
        ));
        let network = Arc::new(
            ClientNetwork::start(network_config, hooks)
                .map_err(|error| format!("Failed to start network client: {error}"))?,
        );
        let mut bridges = BridgeThreadGroup::new();
        let game_network = Arc::clone(&network);
        let game_session = session_lifecycle.clone();
        bridges
            .spawn("roundo-client-game-bridge", move |stop| {
                bridge_game_ecs_events(marionette_ipc, game_session, game_network, stop)
            })
            .map_err(|error| format!("failed to start client game bridge: {error}"))?;
        let resource_network = Arc::clone(&network);
        let resource_session = session_lifecycle;
        bridges
            .spawn("roundo-client-resource-bridge", move |stop| {
                bridge_resource_ecs_events(
                    local_coordinate_ipc,
                    resource_session,
                    resource_network,
                    stop,
                )
            })
            .map_err(|error| format!("failed to start client resource bridge: {error}"))?;

        Ok(Self {
            name: name.to_string(),
            network,
            bridges,
            status,
        })
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

pub(crate) fn network_config(
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
        outbound: roundo_networking::OutboundQueuePolicy::default(),
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

/// Forwards chunk demand to the resource business queue until shutdown or send failure.
///
/// The event that encounters saturation/closure is dropped, and the bridge stops
/// permanently; it does not retry after network reconnection.
fn bridge_resource_ecs_events(
    local_coordinate_ipc: LocalCoordinateClientIpc,
    session_lifecycle: ClientGameSessionLifecycle,
    network: Arc<ClientNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started client resource IPC bridge");
    run_polling_bridge(&stop, || {
        session_lifecycle.with_active(|| {
        let Some(event) = local_coordinate_ipc.try_receive() else {
            return BridgeStep::Idle;
        };
        let message = match event {
            LocalCoordinateClientEvent::SetChunkViewDistance { chunks } => {
                ClientResourceMessage::SetChunkViewDistance { chunks }
            }
        };
        let message_kind = message.kind();
        if let Err(error) = network.send(message) {
            warn!(
                "Stopped client resource bridge after send failure: message={message_kind}, error={error}"
            );
            return BridgeStep::Stop;
        }
        BridgeStep::Forwarded
        }).unwrap_or(BridgeStep::Idle)
    });
    debug!("Stopped client resource IPC bridge");
}
