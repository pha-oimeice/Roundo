//! Immediate serialization and explicit-destination publication.

use super::{
    Network, NetworkEvent, Peer,
    batching::{NO_DEADLINE, Pending},
};
use crate::{
    DataTransferObject, Deadline, NetworkError, NetworkTypeId, SharedEntityId,
    wire::{SharedBytes, SyncBatch, WireUpdate},
};
use bevy::prelude::{Component, Resource};
use std::{collections::HashSet, sync::Arc};

impl Network {
    /// Publishes a component with an optional relative flush deadline.
    /// `None` sets no time-based trigger; batching or explicit flushing can
    /// still submit the update, including alongside other due updates.
    pub fn sync_component<T: Component + DataTransferObject>(
        &mut self,
        targets: &[Peer],
        entity: SharedEntityId,
        value: &T,
        deadline: Option<Deadline>,
    ) -> Result<(), NetworkError> {
        self.submit(targets, Some(entity), value, deadline)
    }

    /// Publishes a resource with an optional relative flush deadline.
    /// `None` sets no time-based trigger; batching or explicit flushing can
    /// still submit the update, including alongside other due updates.
    pub fn sync_resource<T: Resource + DataTransferObject>(
        &mut self,
        targets: &[Peer],
        value: &T,
        deadline: Option<Deadline>,
    ) -> Result<(), NetworkError> {
        self.submit(targets, None, value, deadline)
    }

    fn submit<T: DataTransferObject + 'static>(
        &mut self,
        targets: &[Peer],
        entity: Option<SharedEntityId>,
        value: &T,
        deadline: Option<Deadline>,
    ) -> Result<(), NetworkError> {
        let id = NetworkTypeId::of::<T>();
        if targets.is_empty() {
            return Err(NetworkError::NoTargets);
        }
        let mut unique = HashSet::new();
        for peer in targets {
            if !self.peers.contains_key(peer) {
                return Err(NetworkError::UnknownPeer);
            }
            if !unique.insert(peer) {
                return Err(NetworkError::DuplicateTarget);
            }
        }
        let due = deadline.map_or(NO_DEADLINE, |deadline| {
            self.tick.saturating_add(deadline.ticks.get())
        });
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(NetworkError::TickOverflow)?;
        let previous = self.statistics.get(&id).copied().unwrap_or_default();
        // No clone of T, retained reference, or delayed callback into the World.
        let data = SharedBytes(Arc::new(postcard::to_stdvec(value)?));
        let statistics = previous.with_sample(data.0.len())?;
        let update = WireUpdate {
            type_id: id,
            entity,
            data,
        };
        // Respect only the existing physical message limit. Validate every
        // destination before accepting this update into any destination queue.
        let single_size =
            postcard::experimental::serialized_size(&SyncBatch(vec![update.clone()]))?;
        for peer in targets {
            let state = &self.peers[peer];
            if single_size > state.limit {
                return Err(NetworkError::MessageTooLarge);
            }
            state
                .payload_bytes
                .checked_add(update.data.0.len())
                .ok_or(NetworkError::SizeOverflow)?;
        }
        for peer in targets {
            let state = self.peers.get_mut(peer).expect("validated attached peer");
            // Compare predicted batch sizes before inserting this update. A
            // flush here transfers older owned buffers, never delays sampling.
            if self
                .policy
                .prefer_flush_before(state.payload_bytes, previous.predicted_bytes())
            {
                if let Err(error) = state.dispatch() {
                    self.events.push_back(NetworkEvent::Error {
                        peer: peer.clone(),
                        error,
                    });
                }
            }
            state.payload_bytes += update.data.0.len();
            state.pending.push(Pending {
                due,
                sequence: self.sequence,
                update: update.clone(),
            });
            if self
                .policy
                .should_flush(state.payload_bytes, state.pending.len())
            {
                if let Err(error) = state.dispatch() {
                    self.events.push_back(NetworkEvent::Error {
                        peer: peer.clone(),
                        error,
                    });
                }
            }
        }
        self.sequence = next_sequence;
        self.statistics.insert(id, statistics);
        Ok(())
    }
}
