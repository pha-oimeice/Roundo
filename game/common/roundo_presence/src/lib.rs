//! Presence identities, scene membership, and nearby-entity synchronization.

mod client;
mod server;

use bevy::prelude::Component;

pub use client::{
    ClientJoinableWorld, ClientPlayerMarker, ClientPresenceCommand, ClientPresenceIpc,
    ClientPresenceSettings, LocalPlayerIdentity, RoundoPresenceClientPlugin,
};
pub use roundo_contracts::{
    JoinableWorldId, NearbyJoinableWorld, NearbyPlayer, PlayerId, PlayerState, PresenceSnapshot,
    SceneId,
};
pub use server::{
    PlayerConnection, PlayerScene, PresenceAdmin, PresenceServerCommand, PresenceServerEvent,
    PresenceServerIpc, PresenceServerSet, PresenceServerSettings, RoundoPresenceServerPlugin,
    ServerPlayer, ServerPlayerSnapshot,
};

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
/// Stable network identity attached to a player entity.
pub struct Player {
    pub id: PlayerId,
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
/// Discoverable world metadata exposed through presence snapshots.
pub struct JoinableWorld {
    pub id: JoinableWorldId,
    pub name: String,
}
