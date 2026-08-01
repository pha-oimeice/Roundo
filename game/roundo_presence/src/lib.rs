mod client;
mod server;

use bevy::prelude::Component;

pub use client::{
    ClientJoinableWorld, ClientPlayerMarker, ClientPresenceCommand, ClientPresenceEvent,
    ClientPresenceIpc, ClientPresenceSettings, LocalPlayerIdentity, RoundoPresenceClientPlugin,
};
pub use roundo_networking::{
    JoinableWorldId, NearbyJoinableWorld, NearbyPlayer, PlayerId, PresenceSnapshot,
};
pub use server::{
    PresenceServerCommand, PresenceServerEvent, PresenceServerIpc, PresenceServerSettings,
    RoundoPresenceServerPlugin, ServerPlayer,
};

pub const ARDA_WORLD_ID: JoinableWorldId = JoinableWorldId(1);
pub const ARDA_WORLD_NAME: &str = "Arda";

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Player {
    pub id: PlayerId,
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct JoinableWorld {
    pub id: JoinableWorldId,
    pub name: String,
}
