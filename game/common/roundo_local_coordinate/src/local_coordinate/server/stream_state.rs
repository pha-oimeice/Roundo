//! Connection-scoped Local Coordinate stream state transitions.

use super::*;

/// Per-connection view and the exact state last advertised to that connection.
pub(super) struct PlayerSubscription {
    pub(super) anchor: RenderingAnchorState,
    pub(super) prediction_anchor: PredictionAnchorState,
    pub(super) spawned_coordinates: HashSet<LocalCoordinateId>,
    pub(super) advertised_chunks: HashMap<ChunkId, UpdateVersion>,
    /// Revisions are retained for tombstones so removal is delivered exactly once.
    pub(super) advertised_environment_revisions:
        HashMap<crate::local_coordinate::virtual_chunk::VirtualChunkCoordinate, u64>,
    pub(super) pending_requests: VecDeque<ChunkId>,
}

impl LocalCoordinateServerWorld {
    /// Applies one network-facing command atomically and returns its observable stream events.
    pub(super) fn apply_stream_command(
        &mut self,
        command: LocalCoordinateServerCommand,
    ) -> Vec<LocalCoordinateServerEvent> {
        match command {
            LocalCoordinateServerCommand::SubscribePlayer {
                connection_id,
                player_id,
            } => self.subscribe_player(connection_id, player_id),
            LocalCoordinateServerCommand::UnsubscribePlayer { connection_id } => {
                self.unsubscribe_player(connection_id)
            }
            LocalCoordinateServerCommand::RequestChunks {
                connection_id,
                chunks,
            } => {
                if let Some(subscription) = self.subscriptions.get_mut(&connection_id) {
                    enqueue_chunk_requests(subscription, chunks);
                }
                Vec::new()
            }
            LocalCoordinateServerCommand::SetChunkViewDistance {
                connection_id,
                chunks,
            } => self.set_chunk_view_distance(connection_id, chunks),
            LocalCoordinateServerCommand::UpdateControlledCreaturePosition {
                connection_id,
                position,
            } => self.update_controlled_creature_position(connection_id, position),
        }
    }

    fn subscribe_player(
        &mut self,
        connection_id: ConnectionId,
        player_id: PlayerId,
    ) -> Vec<LocalCoordinateServerEvent> {
        let radius_chunks = self
            .requested_view_distances
            .get(&connection_id)
            .copied()
            .unwrap_or(DEFAULT_CHUNK_VIEW_DISTANCE);
        let anchor = RenderingAnchorState {
            id: ChunkLoadingAnchorId(self.next_anchor_id),
            owner: player_id,
            scene_id: SceneId::S1,
            position: [0.0; 3],
            radius_chunks,
        };
        self.next_anchor_id = self.next_anchor_id.saturating_add(1);
        let prediction_anchor = PredictionAnchorState {
            id: PredictionAnchorId(self.next_prediction_anchor_id),
            owner: player_id,
            scene_id: SceneId::S1,
            position: [0.0; 3],
            radius_chunks: PREDICTION_CHUNK_RADIUS,
        };
        self.next_prediction_anchor_id = self.next_prediction_anchor_id.saturating_add(1);
        self.subscriptions.insert(
            connection_id,
            PlayerSubscription {
                anchor,
                prediction_anchor,
                spawned_coordinates: HashSet::new(),
                advertised_chunks: HashMap::new(),
                advertised_environment_revisions: HashMap::new(),
                pending_requests: VecDeque::new(),
            },
        );
        vec![
            LocalCoordinateServerEvent::RenderingAnchorSpawned {
                connection_id,
                anchor,
            },
            LocalCoordinateServerEvent::PredictionAnchorSpawned {
                connection_id,
                anchor: prediction_anchor,
            },
        ]
    }

    fn unsubscribe_player(
        &mut self,
        connection_id: ConnectionId,
    ) -> Vec<LocalCoordinateServerEvent> {
        let removed_subscription = self.subscriptions.remove(&connection_id);
        let removed_distance = self.requested_view_distances.remove(&connection_id);
        self.derived_svo_jobs
            .retain(|(candidate, _), _| *candidate != connection_id);
        let Some(subscription) = removed_subscription else {
            log::debug!(
                "ignored unsubscribe for unknown Local Coordinate subscription: connection_id={}, had_view_distance={}",
                connection_id.0,
                removed_distance.is_some()
            );
            return Vec::new();
        };
        vec![
            LocalCoordinateServerEvent::RenderingAnchorDespawned {
                connection_id,
                anchor_id: subscription.anchor.id,
            },
            LocalCoordinateServerEvent::PredictionAnchorDespawned {
                connection_id,
                anchor_id: subscription.prediction_anchor.id,
            },
        ]
    }

    fn set_chunk_view_distance(
        &mut self,
        connection_id: ConnectionId,
        chunks: u16,
    ) -> Vec<LocalCoordinateServerEvent> {
        let chunks = chunks.clamp(MIN_CHUNK_VIEW_DISTANCE, MAX_CHUNK_VIEW_DISTANCE);
        self.requested_view_distances.insert(connection_id, chunks);
        let Some(subscription) = self.subscriptions.get_mut(&connection_id) else {
            return Vec::new();
        };
        subscription.anchor.radius_chunks = chunks;
        vec![LocalCoordinateServerEvent::RenderingAnchorUpdated {
            connection_id,
            anchor: subscription.anchor,
        }]
    }

    fn update_controlled_creature_position(
        &mut self,
        _connection_id: ConnectionId,
        _position: [f64; 3],
    ) -> Vec<LocalCoordinateServerEvent> {
        // Creature motion and Chunk Loading Anchor motion are deliberately
        // decoupled. Until an explicit anchor-control policy is introduced,
        // both rendering and prediction anchors remain at the scene origin.
        Vec::new()
    }
}

fn enqueue_chunk_requests(subscription: &mut PlayerSubscription, chunks: Vec<ChunkId>) {
    for chunk in chunks {
        if subscription.advertised_chunks.contains_key(&chunk)
            && !subscription.pending_requests.contains(&chunk)
        {
            subscription.pending_requests.push_back(chunk);
        }
    }
}
