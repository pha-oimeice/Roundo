//! Ordered resource forwarding with bounded retention on transport saturation.

use super::*;
use roundo_contracts::ServerMessage;
use roundo_networking::ServerSendError;
use std::collections::{HashSet, VecDeque};

const MAX_PENDING_MESSAGES: usize = 64;

/// Retains messages until local queue admission, connection closure or shutdown.
/// Per-connection FIFO includes versions and lifecycle events, not just payloads.
/// Other connections in the staging window can advance past a saturated peer;
/// a full window backpressures IPC draining rather than growing another queue.
/// Admission is not a peer acknowledgment; a transport failure ends the session.
pub(super) fn bridge_resource_ecs_events(
    local_coordinate_ipc: LocalCoordinateServerIpc,
    network: Arc<ServerNetwork>,
    stop: Arc<AtomicBool>,
) {
    debug!("Started server resource IPC bridge");
    let mut pending = VecDeque::new();
    run_polling_bridge(&stop, || {
        let mut progressed = false;
        while pending.len() < MAX_PENDING_MESSAGES {
            let Some(event) = local_coordinate_ipc.try_receive() else {
                break;
            };
            let (connection_id, message) = resource_message(event);
            pending.push_back((connection_id, ServerMessage::from(message)));
            progressed = true;
        }
        let mut blocked = HashSet::new();
        for _ in 0..pending.len() {
            let (connection_id, message) = pending.pop_front().expect("fixed staging window");
            if blocked.contains(&connection_id) {
                pending.push_back((connection_id, message));
                continue;
            }
            match network.try_send_to_connection(connection_id, message) {
                Ok(()) | Err(ServerSendError::Closed) => progressed = true,
                Err(ServerSendError::Saturated(message)) => {
                    blocked.insert(connection_id);
                    pending.push_back((connection_id, message));
                }
                Err(ServerSendError::NegotiationMessage) => {
                    unreachable!("resource projection only produces application messages");
                }
            }
        }
        if progressed {
            BridgeStep::Forwarded
        } else {
            // Includes saturation: avoid spinning while retaining the original message.
            BridgeStep::Idle
        }
    });
}

fn resource_message(event: LocalCoordinateServerEvent) -> (ConnectionId, ServerResourceMessage) {
    use LocalCoordinateServerEvent as Event;
    use ServerResourceMessage as Message;
    match event {
        Event::RenderingAnchorSpawned {
            connection_id,
            anchor,
        } => (connection_id, Message::RenderingAnchorSpawned { anchor }),
        Event::RenderingAnchorUpdated {
            connection_id,
            anchor,
        } => (connection_id, Message::RenderingAnchorUpdated { anchor }),
        Event::RenderingAnchorDespawned {
            connection_id,
            anchor_id,
        } => (
            connection_id,
            Message::RenderingAnchorDespawned { anchor_id },
        ),
        Event::PredictionAnchorSpawned {
            connection_id,
            anchor,
        } => (connection_id, Message::PredictionAnchorSpawned { anchor }),
        Event::PredictionAnchorUpdated {
            connection_id,
            anchor,
        } => (connection_id, Message::PredictionAnchorUpdated { anchor }),
        Event::PredictionAnchorDespawned {
            connection_id,
            anchor_id,
        } => (
            connection_id,
            Message::PredictionAnchorDespawned { anchor_id },
        ),
        Event::EnvironmentOverrides {
            connection_id,
            overrides,
        } => (
            connection_id,
            Message::VirtualChunkEnvironmentOverrides { overrides },
        ),
        Event::Spawned {
            connection_id,
            local_coordinate_id,
        } => (
            connection_id,
            Message::LocalCoordinateSpawned {
                local_coordinate_id,
            },
        ),
        Event::Despawned {
            connection_id,
            local_coordinate_id,
        } => (
            connection_id,
            Message::LocalCoordinateDespawned {
                local_coordinate_id,
            },
        ),
        Event::ChunkLoaded {
            connection_id,
            chunk,
            payload,
        } => (
            connection_id,
            Message::LocalCoordinateChunk {
                chunk,
                edge_length: CHUNK_EDGE_LENGTH as u16,
                svo: payload,
            },
        ),
        Event::ChunkVersions {
            connection_id,
            chunks,
        } => (
            connection_id,
            Message::LocalCoordinateChunkVersions { chunks },
        ),
        Event::ChunkUnloaded {
            connection_id,
            local_coordinate_id,
            coordinate,
        } => (
            connection_id,
            Message::LocalCoordinateChunkUnloaded {
                local_coordinate_id,
                coordinate,
            },
        ),
    }
}
