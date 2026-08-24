mod client;
mod server;

use super::shared::{ControllerAction, EventController};
use bevy::prelude::{Entity, Message};
use roundo_networking::protocol::ControllerCommand;
pub use roundo_networking::protocol::{DestroyBlockControllerAction, PlaceBlockControllerAction};

pub(super) use self::client::route_block_interactions;
pub(super) use self::server::accept_block_interactions;

pub type DestroyBlockController = EventController<DestroyBlockControllerAction>;
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
pub enum BlockInteraction {
    Destroy,
    Place { voxel_id: u32 },
}

#[derive(Message, Clone, Copy, Debug, Eq, PartialEq)]
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
