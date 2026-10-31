//! Typed semantic interaction data, independent of the legacy input adapter.

use crate::InteractionAttemptId;
use bevy::prelude::{App, IVec3, Plugin};
use roundo_life::{AuthoritativeFixedTick, EventData, EventShape, LifeData, LifeDataTypeKey};
use roundo_local_coordinate::{AtomicVoxelId, EMPTY_VOXEL_ID, LocalCoordinateId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInteractionDataError {
    EmptyPlacement,
    InvalidTerminal,
}

/// Semantic operation only: no ray, device, Controller or success declaration.
/// Nonzero placement is a source invariant; registry/placeable checks belong to Execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInteractionOperation(Operation);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Destroy,
    Place(AtomicVoxelId),
}

impl BlockInteractionOperation {
    pub const fn destroy() -> Self {
        Self(Operation::Destroy)
    }

    pub fn place(voxel: AtomicVoxelId) -> Result<Self, BlockInteractionDataError> {
        if voxel == EMPTY_VOXEL_ID {
            return Err(BlockInteractionDataError::EmptyPlacement);
        }
        Ok(Self(Operation::Place(voxel)))
    }

    pub const fn replacement(self) -> AtomicVoxelId {
        match self.0 {
            Operation::Destroy => EMPTY_VOXEL_ID,
            Operation::Place(voxel) => voxel,
        }
    }

    pub const fn is_destroy(self) -> bool {
        matches!(self.0, Operation::Destroy)
    }
}

/// Both operations occupy this one Event FIFO, preserving Destroy/Place order.
/// Acceptance means publication only, never a world edit or terminal outcome.
/// An unread Command expires normally; Execution does not track its cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInteractionCommandEvent {
    operation: BlockInteractionOperation,
}

impl BlockInteractionCommandEvent {
    pub const fn new(operation: BlockInteractionOperation) -> Self {
        Self { operation }
    }

    pub const fn operation(self) -> BlockInteractionOperation {
        self.operation
    }
}

roundo_life::declare_event_data!(
    BlockInteractionCommandEvent,
    "roundo.block-interaction.command.v2",
    |_| std::mem::size_of::<BlockInteractionCommandEvent>()
);

/// Owned description of the resolved target, not edit/incarnation authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInteractionTarget {
    pub coordinate: LocalCoordinateId,
    pub position: IVec3,
}

/// Terminal domain result. Applied must be translated from a real canonical
/// completion by the Execution adapter; these values alone are not a commit proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockInteractionOutcome {
    Applied {
        previous: AtomicVoxelId,
        current: AtomicVoxelId,
        revision_before: u64,
        revision_after: u64,
    },
    NoHit,
    InvalidVoxel,
    Occupied,
    SelfIntersection,
    TargetChanged,
    NotReady,
    TargetUnavailable,
}

/// A bounded, owned terminal fact written by Execution, not by Intent.
/// No constructor performs a voxel edit or publishes anything to Life.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInteractionExperienceEvent {
    attempt: InteractionAttemptId,
    completed_tick: AuthoritativeFixedTick,
    operation: BlockInteractionOperation,
    target: Option<BlockInteractionTarget>,
    outcome: BlockInteractionOutcome,
}

impl BlockInteractionExperienceEvent {
    /// Validates the shape of a terminal fact. In particular, Applied requires
    /// a target, an actual value change, a strictly newer revision, and values
    /// consistent with the requested operation. This is not provenance validation.
    pub fn new(
        attempt: InteractionAttemptId,
        completed_tick: AuthoritativeFixedTick,
        operation: BlockInteractionOperation,
        target: Option<BlockInteractionTarget>,
        outcome: BlockInteractionOutcome,
    ) -> Result<Self, BlockInteractionDataError> {
        let value = Self {
            attempt,
            completed_tick,
            operation,
            target,
            outcome,
        };
        if !EventData::validate(&value) {
            return Err(BlockInteractionDataError::InvalidTerminal);
        }
        Ok(value)
    }

    pub const fn attempt(self) -> InteractionAttemptId {
        self.attempt
    }
    pub const fn completed_tick(self) -> AuthoritativeFixedTick {
        self.completed_tick
    }
    pub const fn operation(self) -> BlockInteractionOperation {
        self.operation
    }
    pub const fn target(self) -> Option<BlockInteractionTarget> {
        self.target
    }
    pub const fn outcome(self) -> BlockInteractionOutcome {
        self.outcome
    }
}

impl roundo_life::__private::Sealed for BlockInteractionExperienceEvent {}
impl LifeData for BlockInteractionExperienceEvent {
    type Shape = EventShape;
    const KEY: LifeDataTypeKey =
        LifeDataTypeKey::from_source("roundo.block-interaction.experience.v1");
}
impl EventData for BlockInteractionExperienceEvent {
    fn retained_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn validate(&self) -> bool {
        match self.outcome {
            BlockInteractionOutcome::Applied {
                previous,
                current,
                revision_before,
                revision_after,
            } => {
                self.target.is_some()
                    && previous != current
                    && revision_after > revision_before
                    && current == self.operation.replacement()
                    && (self.operation.is_destroy() || previous == EMPTY_VOXEL_ID)
            }
            BlockInteractionOutcome::Occupied
            | BlockInteractionOutcome::SelfIntersection
            | BlockInteractionOutcome::TargetChanged => self.target.is_some(),
            BlockInteractionOutcome::NoHit => self.target.is_none(),
            _ => true,
        }
    }
}

/// Registers source schemas and their private ID source only. No Factory call,
/// route, demand, application instance, input adapter or execution system is installed.
pub struct BlockInteractionLifeDataPlugin;

impl Plugin for BlockInteractionLifeDataPlugin {
    fn build(&self, app: &mut App) {
        roundo_life::register_event_type::<crate::ItemUseCommandEvent>(app)
            .expect("item use Command schema must register before catalogue close");
        roundo_life::register_event_type::<BlockInteractionCommandEvent>(app)
            .expect("interaction Command schema must register before catalogue close");
        roundo_life::register_event_type::<BlockInteractionExperienceEvent>(app)
            .expect("interaction Experience schema must register before catalogue close");
        app.init_resource::<crate::attempt::InteractionSpaces>();
    }
}
