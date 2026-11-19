//! World-local Player presence and scene membership.
mod server;
use bevy::prelude::Component;
pub use roundo_contracts::{PlayerId, SceneId};
pub use server::{PlayerScene, PresenceAdmin, ServerPlayer, ServerPlayerSnapshot};

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Player {
    pub id: PlayerId,
}
