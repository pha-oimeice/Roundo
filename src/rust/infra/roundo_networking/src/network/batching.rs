//! Deadline ordering, FIFO batch construction and flush initiation.

use super::{Network, NetworkEvent, peer::PeerState};
use crate::{
    NetworkError,
    wire::{WireUpdate, split_batches},
};
use std::cmp::Ordering;

/// Infinite waiting time, also used when a computed due tick reaches u64::MAX.
pub(super) const NO_DEADLINE: u64 = u64::MAX;

pub(super) struct Pending {
    pub(super) due: u64,
    pub(super) sequence: u64,
    pub(super) update: WireUpdate,
}
impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        (self.due, self.sequence) == (other.due, other.sequence)
    }
}
impl Eq for Pending {}
impl PartialOrd for Pending {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Pending {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse due tick/order for this max heap. NO_DEADLINE naturally
        // follows every finite deadline without separate ordering branches.
        (other.due, other.sequence).cmp(&(self.due, self.sequence))
    }
}

impl PeerState {
    pub(super) fn dispatch(&mut self) -> Result<usize, NetworkError> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        // Deadline order selects when to flush, not the order of state updates.
        // Keep FIFO within the batch so different deadlines cannot reverse two
        // accepted updates to the same component.
        let mut pending: Vec<_> = self.pending.drain().collect();
        pending.sort_unstable_by_key(|item| item.sequence);
        self.payload_bytes = 0;
        let count = pending.len();
        let batches = split_batches(
            pending.into_iter().map(|item| item.update).collect(),
            self.limit,
        )?;
        self.outgoing
            .send(batches)
            .map_err(|_| NetworkError::WorkerStopped)?;
        Ok(count)
    }
}

impl Network {
    /// Flushes every peer whose earliest deadline has arrived. All queued
    /// updates for that peer, including those without deadlines, are included
    /// in original submission order. Completion/errors arrive as NetworkEvent;
    /// this never waits for QUIC.
    pub fn flush_due(&mut self) {
        for (peer, state) in &mut self.peers {
            if state
                .pending
                .peek()
                .is_some_and(|item| item.due != NO_DEADLINE && item.due <= self.tick)
            {
                if let Err(error) = state.dispatch() {
                    self.events.push_back(NetworkEvent::Error {
                        peer: peer.clone(),
                        error,
                    });
                }
            }
        }
    }

    /// Explicit application-layer flush, independent of deadline/size triggers.
    pub fn flush(&mut self) {
        for (peer, state) in &mut self.peers {
            if let Err(error) = state.dispatch() {
                self.events.push_back(NetworkEvent::Error {
                    peer: peer.clone(),
                    error,
                });
            }
        }
    }
}
