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
    DEFAULT_S0_ROOM_SIZE, DEFAULT_S1_EDGE_LENGTH, PlayerConnection, PlayerScene,
    PresenceServerCommand, PresenceServerEvent, PresenceServerIpc, PresenceServerSet,
    PresenceServerSettings, RoundoPresenceServerPlugin, ServerPlayer, ServerSceneWorlds,
    TorusSpace,
};

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Player {
    pub id: PlayerId,
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct JoinableWorld {
    pub id: JoinableWorldId,
    pub name: String,
}
