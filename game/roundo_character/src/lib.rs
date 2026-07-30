mod client;
mod server;

use bevy::prelude::{Commands, Component, Entity, Message, Transform};

pub use client::{
    ClientCharacterCommand, ClientCharacterEvent, ClientCharacterIpc, ControlledCharacter,
    RoundoCharacterClientPlugin,
};
pub use roundo_networking::{CharacterId, CharacterSnapshot};
pub use server::{
    CharacterServerCommand, CharacterServerEvent, CharacterServerIpc, RoundoCharacterServerPlugin,
    ServerCharacter,
};

pub const DEFAULT_CHARACTER_ID: CharacterId = CharacterId(1);

/// Stable tag shared by server and client character entities.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct Character {
    pub id: CharacterId,
}

/// ECS spawn request for an otherwise property-free character.
#[derive(Message, Clone, Debug)]
pub struct SpawnCharacter {
    pub id: CharacterId,
    pub transform: Transform,
}

/// Direct spawn interface for systems that already own [`Commands`].
pub fn spawn_character(commands: &mut Commands, request: SpawnCharacter) -> Entity {
    commands
        .spawn((
            Character { id: request.id },
            ServerCharacter,
            request.transform,
        ))
        .id()
}
