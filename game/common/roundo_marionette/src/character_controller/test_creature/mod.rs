//! Development-only semantic intent for spawning an unowned test Creature.

#[cfg(feature = "dev")]
mod client;
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

#[cfg(feature = "dev")]
pub(super) use client::route_spawn_test_creature;
pub(super) use server::accept_spawn_test_creature;

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct SpawnTestCreatureControllerMessage {
    pub entity: Entity,
    pub command: ControllerCommand<SpawnTestCreatureAction>,
}

#[derive(Message, Clone, Copy, Debug, Eq, PartialEq)]
/// Accepted semantic intent resolved to the authenticated Controller entity.
pub struct SpawnTestCreatureIntent {
    pub controller: Entity,
}
