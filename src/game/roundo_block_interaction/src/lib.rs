//! Passive block-related item data and local correlation identities.
//!
//! This module installs no gameplay operation, item-use dispatch or world-edit
//! system. Explicit voxel placement/removal uses
//! `roundo_local_coordinate::LocalVoxelEdits`.

mod attempt;
mod item_data;

pub use attempt::{
    InteractionActivationId, InteractionAttemptAllocator, InteractionAttemptId,
    InteractionAttemptIds, InteractionIdError, InteractionIdsPlugin,
};
pub use item_data::{BlockDestructionPermit, BlockItemSchemas, BlockPlacementPermit};
