//! Block interaction schemas and passive item permission sources.
//!
//! The schema-only plugin activates no instances or world execution. Player
//! behavior belongs to the Execution adapter, which checks current item data
//! before submitting a canonical world edit.

mod attempt;
mod budget;
mod command_data;
mod item_data;
mod record;

pub use attempt::{
    InteractionActivationId, InteractionAttemptAllocator, InteractionAttemptId,
    InteractionAttemptIds, InteractionIdError,
};
pub use budget::{
    BlockInteractionBudgetError, BlockInteractionBudgetLimits, BlockInteractionBudgets,
};
pub use command_data::{
    BlockInteractionCommandEvent, BlockInteractionDataError, BlockInteractionExperienceEvent,
    BlockInteractionLifeDataPlugin, BlockInteractionOperation, BlockInteractionOutcome,
    BlockInteractionTarget,
};
pub use item_data::{
    BlockDestructionPermit, BlockItemSchemas, BlockPlacementPermit, ItemUseCommandEvent,
};
pub use record::{
    BlockInteractionRecord, BlockInteractionRecordError, BlockInteractionRecordUpdate,
};

use bevy::prelude::{Component, Vec3};

/// Marks the entity whose Creature pose is authoritative for block interaction.
#[derive(Component)]
pub struct BlockInteractionPose;

/// Composition-owned body-local ray-origin offset from the logical entity position.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct BlockInteractionOriginOffset(pub Vec3);
