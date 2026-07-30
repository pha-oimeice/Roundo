use crate::{Character, DEFAULT_CHARACTER_ID, SpawnCharacter, spawn_character};
use bevy::prelude::{
    App, Commands, Component, FixedUpdate, IntoScheduleConfigs, MessageReader, Plugin, Query, Res,
    ResMut, Resource, Startup, Transform,
};
use roundo_networking::{CharacterId, CharacterSnapshot, ConnectionId, UserSession};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet};

pub type CharacterServerIpc =
    CrossbeamThreadPipeEndpointA<CharacterServerCommand, CharacterServerEvent>;

#[derive(Clone)]
pub struct RoundoCharacterServerPlugin {
    pipe: CrossbeamThreadPipe<CharacterServerCommand, CharacterServerEvent>,
}

impl RoundoCharacterServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> CharacterServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for RoundoCharacterServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for RoundoCharacterServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SpawnCharacter>()
            .init_resource::<CharacterControlRegistry>()
            .insert_resource(CharacterServerPipe(self.pipe.endpoint_b()))
            .add_systems(Startup, spawn_default_character)
            .add_systems(
                FixedUpdate,
                (
                    spawn_requested_characters,
                    process_control_requests,
                    publish_character_snapshots,
                )
                    .chain(),
            );
    }
}

#[derive(Clone, Debug)]
pub enum CharacterServerCommand {
    RequestControl {
        connection_id: ConnectionId,
        user_session: UserSession,
        character_id: CharacterId,
    },
}

#[derive(Clone, Debug)]
pub enum CharacterServerEvent {
    Snapshot(CharacterSnapshot),
    ControlGranted {
        connection_id: ConnectionId,
        character_id: CharacterId,
    },
}

/// Explicit server-side tag used to distinguish authoritative characters.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ServerCharacter;

#[derive(Resource, Clone)]
struct CharacterServerPipe(
    CrossbeamThreadPipeEndpointB<CharacterServerCommand, CharacterServerEvent>,
);

#[derive(Resource, Default)]
struct CharacterControlRegistry {
    owners: HashMap<CharacterId, UserSession>,
}

fn spawn_default_character(mut commands: Commands) {
    spawn_character(
        &mut commands,
        SpawnCharacter {
            id: DEFAULT_CHARACTER_ID,
            transform: Transform::from_xyz(0.0, 0.0, 0.0),
        },
    );
}

fn spawn_requested_characters(
    mut commands: Commands,
    mut requests: MessageReader<SpawnCharacter>,
    characters: Query<&Character>,
) {
    let mut known_ids = characters
        .iter()
        .map(|character| character.id)
        .collect::<HashSet<_>>();
    for request in requests.read() {
        if !known_ids.insert(request.id) {
            continue;
        }
        spawn_character(&mut commands, request.clone());
    }
}

fn process_control_requests(
    pipe: Res<CharacterServerPipe>,
    mut registry: ResMut<CharacterControlRegistry>,
    characters: Query<&Character>,
) {
    while let Some(command) = pipe.0.try_receive() {
        let CharacterServerCommand::RequestControl {
            connection_id,
            user_session,
            character_id,
        } = command;
        if !characters
            .iter()
            .any(|character| character.id == character_id)
        {
            continue;
        }

        let owner = registry.owners.entry(character_id).or_insert(user_session);
        if *owner == user_session {
            let _ = pipe.0.try_send(CharacterServerEvent::ControlGranted {
                connection_id,
                character_id,
            });
        }
    }
}

fn publish_character_snapshots(
    pipe: Res<CharacterServerPipe>,
    characters: Query<(&Character, &Transform)>,
) {
    for (character, transform) in &characters {
        let _ = pipe
            .0
            .try_send(CharacterServerEvent::Snapshot(CharacterSnapshot {
                character_id: character.id,
                translation: transform.translation.to_array(),
                rotation: transform.rotation.to_array(),
                scale: transform.scale.to_array(),
            }));
    }
}
