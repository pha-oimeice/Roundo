//! Development-only semantic intent for spawning an unowned test Creature.

mod server;

use super::shared::{ControllerAction, EventController};
use bevy::prelude::{Entity, Message};
use roundo_contracts::ControllerCommand;
pub use roundo_contracts::SpawnTestCreatureAction;

pub type SpawnTestCreatureController = EventController<SpawnTestCreatureAction>;

impl ControllerAction for SpawnTestCreatureAction {
    fn is_valid(self) -> bool {
        true
    }
}

pub(super) use server::accept_spawn_test_creature;

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct SpawnTestCreatureControllerMessage {
    pub entity: Entity,
    pub command: ControllerCommand<SpawnTestCreatureAction>,
}

#[derive(Message, Clone, Copy, Debug, Eq, PartialEq)]
/// Accepted semantic intent resolved to the local Controller entity.
pub struct SpawnTestCreatureIntent {
    pub controller: Entity,
}
