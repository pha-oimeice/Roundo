//! Authenticated inbound game/resource hooks for the current session.

use super::*;

pub struct ClientHooksAdapter {
    local_coordinate_ipc: LocalCoordinateClientIpc,
    session_lifecycle: ClientGameSessionLifecycle,
    resource_fingerprint: ResourceCatalogFingerprint,
    on_connection_event: Box<dyn Fn(ClientConnectionEvent) + Send + Sync>,
}

impl ClientHooksAdapter {
    pub fn new(
        local_coordinate_ipc: LocalCoordinateClientIpc,
        session_lifecycle: ClientGameSessionLifecycle,
        resource_fingerprint: ResourceCatalogFingerprint,
        on_connection_event: impl Fn(ClientConnectionEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            local_coordinate_ipc,
            session_lifecycle,
            resource_fingerprint,
            on_connection_event: Box::new(on_connection_event),
        }
    }
}

impl ClientHooks for ClientHooksAdapter {
    fn resource_catalog_fingerprint(&self) -> ResourceCatalogFingerprint {
        self.resource_fingerprint
    }

    fn on_server_game_message(&self, message: ServerGameMessage) {
        self.session_lifecycle.route_game(message);
    }

    fn on_server_resource_message(&self, message: ServerResourceMessage) {
        if self
            .session_lifecycle
            .with_active(|| {
                let message_kind = message.kind();
                let command = resource_command(message);
                if let Err(command) = self.local_coordinate_ipc.try_send(command) {
                    log_resource_drop(message_kind, command);
                }
            })
            .is_none()
        {
            warn!("Dropped server resource message outside an active client session");
        }
    }

    fn on_connection_established(&self) {
        let epoch = self.session_lifecycle.begin();
        info!("Client game connection established: session_epoch={epoch}");
        (self.on_connection_event)(ClientConnectionEvent::Established);
    }

    fn on_connection_lost(&self) {
        let epoch = self.session_lifecycle.end();
        warn!("Client game connection lost; waiting to reconnect; session_epoch={epoch:?}");
        (self.on_connection_event)(ClientConnectionEvent::Lost);
    }

    fn on_connection_error(&self, error: &NetworkError) {
        warn!("Client game connection error: {error}");
        (self.on_connection_event)(ClientConnectionEvent::Error(error.to_string()));
    }
}

/// Projects authoritative wire values without sending IPC or validating voxel IDs.
/// An unusable Chunk shape is fatal; network-task panic isolation must not turn
/// malformed authoritative data into a silently missing Chunk.
fn resource_command(message: ServerResourceMessage) -> LocalCoordinateClientCommand {
    use LocalCoordinateClientCommand as Command;
    use ServerResourceMessage as Message;
    match message {
        Message::RenderingAnchorSpawned { anchor } => Command::RenderingAnchorSpawned(anchor),
        Message::RenderingAnchorUpdated { anchor } => Command::RenderingAnchorUpdated(anchor),
        Message::RenderingAnchorDespawned { anchor_id } => {
            Command::RenderingAnchorDespawned(anchor_id)
        }
        Message::PredictionAnchorSpawned { anchor } => Command::PredictionAnchorSpawned(anchor),
        Message::PredictionAnchorUpdated { anchor } => Command::PredictionAnchorUpdated(anchor),
        Message::PredictionAnchorDespawned { anchor_id } => {
            Command::PredictionAnchorDespawned(anchor_id)
        }
        Message::VirtualChunkEnvironmentOverrides { overrides } => {
            Command::EnvironmentOverrides(overrides)
        }
        Message::LocalCoordinateSpawned {
            local_coordinate_id,
        } => Command::Spawn(local_coordinate_id),
        Message::LocalCoordinateDespawned {
            local_coordinate_id,
        } => Command::Despawn(local_coordinate_id),
        Message::LocalCoordinateChunkVersions { chunks } => Command::VersionUpdates(chunks),
        Message::LocalCoordinateChunk {
            chunk,
            edge_length,
            svo: payload,
        } => {
            if usize::from(edge_length) != CHUNK_EDGE_LENGTH {
                log::error!(
                    "cannot consume authoritative LocalCoordinateChunk: local_coordinate_id={}, coordinate={:?}, edge_length={}, expected_edge_length={}",
                    chunk.local_coordinate_id.0,
                    chunk.coordinate,
                    edge_length,
                    CHUNK_EDGE_LENGTH
                );
                std::process::abort();
            }
            Command::LoadChunk { chunk, payload }
        }
        Message::LocalCoordinateChunkUnloaded {
            local_coordinate_id,
            coordinate,
        } => Command::UnloadChunk {
            local_coordinate_id,
            coordinate,
        },
    }
}

/// Preserves the existing per-category diagnostics; conversion and a common
/// IPC send do not imply a common failure-reporting or retry policy.
fn log_resource_drop(message_kind: &'static str, command: LocalCoordinateClientCommand) {
    use LocalCoordinateClientCommand as Command;
    match command {
        Command::RenderingAnchorSpawned(_) | Command::RenderingAnchorUpdated(_) => {
            warn!("Dropped {message_kind} message: destination=local_coordinate_ecs");
        }
        Command::Spawn(local_coordinate_id) | Command::Despawn(local_coordinate_id) => {
            warn!(
                "Dropped {message_kind} message: destination=local_coordinate_ecs, local_coordinate_id={}",
                local_coordinate_id.0
            );
        }
        Command::VersionUpdates(chunks) => {
            let chunk_count = chunks.len();
            warn!(
                "Dropped {message_kind} message: destination=local_coordinate_ecs, chunk_count={chunk_count}"
            );
        }
        Command::LoadChunk { chunk, .. } => {
            let coordinate = chunk.coordinate;
            warn!(
                "Dropped {message_kind} message: destination=local_coordinate_ecs, local_coordinate_id={}, coordinate={coordinate:?}",
                chunk.local_coordinate_id.0
            );
        }
        Command::UnloadChunk {
            local_coordinate_id,
            coordinate,
        } => {
            warn!(
                "Dropped {message_kind} message: destination=local_coordinate_ecs, local_coordinate_id={}, coordinate={coordinate:?}",
                local_coordinate_id.0
            );
        }
        Command::RenderingAnchorDespawned(_)
        | Command::PredictionAnchorSpawned(_)
        | Command::PredictionAnchorUpdated(_)
        | Command::PredictionAnchorDespawned(_)
        | Command::EnvironmentOverrides(_) => {}
        Command::BeginSession { .. } | Command::EndSession { .. } => {
            unreachable!("resource projection cannot produce session lifecycle commands");
        }
    }
}
