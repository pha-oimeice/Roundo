//! Validated limits for Command intake and the entire terminal-result pipeline.

use crate::{BlockInteractionCommandEvent, BlockInteractionExperienceEvent};
use roundo_life::EventPolicy;

/// Explicit host choices; this unvalidated input cannot be installed as a budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInteractionBudgetLimits {
    pub command_entries: usize,
    pub command_bytes: usize,
    pub consume_per_tick: usize,
    pub edits_per_tick: usize,
    pub pending_completions: usize,
    pub outbox_entries: usize,
    pub publish_per_tick: usize,
    pub experience_entries: usize,
    pub experience_bytes: usize,
    pub record_entries: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInteractionBudgetError {
    ZeroLimit,
    ByteOverflow,
    InsufficientCommandBytes,
    InsufficientConsumption,
    InsufficientPending,
    InsufficientOutbox,
    InsufficientExperienceCapacity,
}

/// Validated configuration, not a reservation or authorization capability.
/// Later submission must reserve an outbox slot before publishing a Command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInteractionBudgets(BlockInteractionBudgetLimits);

impl BlockInteractionBudgets {
    pub fn new(limits: BlockInteractionBudgetLimits) -> Result<Self, BlockInteractionBudgetError> {
        use BlockInteractionBudgetError::*;
        if [
            limits.command_entries,
            limits.command_bytes,
            limits.consume_per_tick,
            limits.edits_per_tick,
            limits.pending_completions,
            limits.outbox_entries,
            limits.publish_per_tick,
            limits.experience_entries,
            limits.experience_bytes,
            limits.record_entries,
        ]
        .contains(&0)
        {
            return Err(ZeroLimit);
        }
        let command_bytes = limits
            .command_entries
            .checked_mul(std::mem::size_of::<BlockInteractionCommandEvent>())
            .ok_or(ByteOverflow)?;
        let experience_bytes = limits
            .experience_entries
            .checked_mul(std::mem::size_of::<BlockInteractionExperienceEvent>())
            .ok_or(ByteOverflow)?;
        if limits.command_bytes < command_bytes {
            return Err(InsufficientCommandBytes);
        }
        if limits.consume_per_tick < limits.command_entries {
            return Err(InsufficientConsumption);
        }
        if limits.pending_completions < limits.command_entries {
            return Err(InsufficientPending);
        }
        if limits.outbox_entries < limits.pending_completions {
            return Err(InsufficientOutbox);
        }
        if limits.experience_entries < limits.publish_per_tick
            || limits.experience_bytes < experience_bytes
        {
            return Err(InsufficientExperienceCapacity);
        }
        Ok(Self(limits))
    }

    /// Returns an owned snapshot; changing it does not alter the validated budget.
    pub const fn limits(self) -> BlockInteractionBudgetLimits {
        self.0
    }

    pub fn command_policy(self) -> EventPolicy {
        EventPolicy::new(self.0.command_entries, self.0.command_bytes, 1, 1)
            .expect("validated interaction Command budget")
    }

    pub fn experience_policy(self) -> EventPolicy {
        EventPolicy::new(self.0.experience_entries, self.0.experience_bytes, 1, 1)
            .expect("validated interaction Experience budget")
    }
}
