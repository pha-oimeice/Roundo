//! Bounded owned terminal records, independent of Event reader cursors.

use crate::{BlockInteractionExperienceEvent, InteractionActivationId, InteractionAttemptId};
use bevy::prelude::Component;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInteractionRecordError {
    ZeroCapacity,
    WrongActivation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInteractionRecordUpdate {
    Inserted,
    Duplicate,
    TooOld,
}

/// Keeps the greatest attempt sequences, not an unbounded audit log.
/// Eviction advances a floor: even after eviction an old terminal cannot return.
/// Out-of-order completions above that floor remain acceptable. For a retained
/// attempt, the first terminal wins; later/older facts never replace it.
#[derive(Component)]
pub struct BlockInteractionRecord {
    activation: InteractionActivationId,
    capacity: usize,
    evicted_through: u64,
    entries: BTreeMap<u64, BlockInteractionExperienceEvent>,
}

impl BlockInteractionRecord {
    pub fn new(
        activation: InteractionActivationId,
        capacity: usize,
    ) -> Result<Self, BlockInteractionRecordError> {
        if capacity == 0 {
            return Err(BlockInteractionRecordError::ZeroCapacity);
        }
        Ok(Self {
            activation,
            capacity,
            evicted_through: 0,
            entries: BTreeMap::new(),
        })
    }

    /// Copies a terminal already read through the Record Body's native reader.
    /// This is not publication, world editing, or cross-Body authorization.
    pub fn accept(
        &mut self,
        value: BlockInteractionExperienceEvent,
    ) -> Result<BlockInteractionRecordUpdate, BlockInteractionRecordError> {
        if value.attempt().activation() != self.activation {
            return Err(BlockInteractionRecordError::WrongActivation);
        }
        let sequence = value.attempt().sequence();
        if sequence <= self.evicted_through {
            return Ok(BlockInteractionRecordUpdate::TooOld);
        }
        if self.entries.contains_key(&sequence) {
            return Ok(BlockInteractionRecordUpdate::Duplicate);
        }
        // Reject late values older than the retained window without momentarily
        // exceeding the host's memory-entry limit.
        if self.entries.len() == self.capacity {
            let lowest = *self
                .entries
                .first_key_value()
                .expect("positive full capacity")
                .0;
            if sequence < lowest {
                self.evicted_through = self.evicted_through.max(sequence);
                return Ok(BlockInteractionRecordUpdate::TooOld);
            }
            self.entries.pop_first();
            self.evicted_through = lowest;
        }
        self.entries.insert(sequence, value);
        Ok(BlockInteractionRecordUpdate::Inserted)
    }

    pub fn get(&self, attempt: InteractionAttemptId) -> Option<BlockInteractionExperienceEvent> {
        if attempt.activation() != self.activation {
            return None;
        }
        self.entries.get(&attempt.sequence()).copied()
    }

    /// Owned, sequence-ordered snapshot; readers do not consume one another's data.
    pub fn snapshot(&self) -> Vec<BlockInteractionExperienceEvent> {
        self.entries.values().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}
