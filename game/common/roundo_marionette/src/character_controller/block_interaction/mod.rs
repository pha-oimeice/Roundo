//! Sequenced block-destruction and placement controller events.

mod client;
mod server;

use super::shared::{ControllerAction, EventController};
use bevy::prelude::{Entity, Message};
use roundo_contracts::ControllerCommand;
pub use roundo_contracts::{DestroyBlockControllerAction, PlaceBlockControllerAction};

pub(super) use self::client::route_block_interactions;
pub(super) use self::server::accept_block_interactions;

/// Rejects duplicate or stale destruction commands by sequence.
pub type DestroyBlockController = EventController<DestroyBlockControllerAction>;
/// Rejects invalid, duplicate, or stale placement commands.
pub type PlaceBlockController = EventController<PlaceBlockControllerAction>;

impl ControllerAction for DestroyBlockControllerAction {
    fn is_valid(self) -> bool {
        true
    }
}

impl ControllerAction for PlaceBlockControllerAction {
    fn is_valid(self) -> bool {
        self.voxel_id != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Validated world-edit intent emitted by a player controller.
pub enum BlockInteraction {
    Destroy,
    Place { voxel_id: u32 },
}

#[derive(Message, Clone, Copy, Debug, Eq, PartialEq)]
/// Routes a validated interaction to the target player entity.
pub struct BlockInteractionMessage {
    pub entity: Entity,
    pub interaction: BlockInteraction,
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct DestroyBlockControllerMessage {
    pub entity: Entity,
    pub command: ControllerCommand<DestroyBlockControllerAction>,
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct PlaceBlockControllerMessage {
    pub entity: Entity,
    pub command: ControllerCommand<PlaceBlockControllerAction>,
}
