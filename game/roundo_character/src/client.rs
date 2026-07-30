use crate::Character;
use bevy::{
    mesh::Meshable,
    prelude::{
        App, Assets, Click, Color, Commands, Component, Mesh, Mesh3d, MeshMaterial3d, On, Plugin,
        Pointer, Quat, Query, Res, ResMut, Resource, Sphere, StandardMaterial, Transform, Update,
        Vec3,
    },
};
use roundo_networking::{CharacterId, CharacterSnapshot};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::HashMap;

pub type ClientCharacterIpc =
    CrossbeamThreadPipeEndpointA<ClientCharacterCommand, ClientCharacterEvent>;

#[derive(Clone)]
pub struct RoundoCharacterClientPlugin {
    pipe: CrossbeamThreadPipe<ClientCharacterCommand, ClientCharacterEvent>,
}

impl RoundoCharacterClientPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> ClientCharacterIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for RoundoCharacterClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for RoundoCharacterClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientCharacterRegistry>()
            .insert_resource(ClientCharacterPipe(self.pipe.endpoint_b()))
            .add_systems(Update, apply_character_commands);
    }
}

#[derive(Clone, Debug)]
pub enum ClientCharacterCommand {
    Snapshot(CharacterSnapshot),
    ControlGranted { character_id: CharacterId },
}

#[derive(Clone, Debug)]
pub enum ClientCharacterEvent {
    RequestControl { character_id: CharacterId },
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ControlledCharacter;

#[derive(Resource, Clone)]
struct ClientCharacterPipe(
    CrossbeamThreadPipeEndpointB<ClientCharacterCommand, ClientCharacterEvent>,
);

#[derive(Resource, Default)]
struct ClientCharacterRegistry {
    entities: HashMap<CharacterId, bevy::prelude::Entity>,
    mesh: Option<bevy::prelude::Handle<Mesh>>,
    material: Option<bevy::prelude::Handle<StandardMaterial>>,
}

fn apply_character_commands(
    mut commands: Commands,
    pipe: Res<ClientCharacterPipe>,
    mut registry: ResMut<ClientCharacterRegistry>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientCharacterCommand::Snapshot(snapshot) => {
                let transform = snapshot_transform(snapshot);
                if let Some(entity) = registry.entities.get(&snapshot.character_id).copied() {
                    if let Ok(mut existing_transform) = transforms.get_mut(entity) {
                        *existing_transform = transform;
                    }
                    continue;
                }

                if registry.mesh.is_none() {
                    registry.mesh = Some(meshes.add(Sphere::new(1.0).mesh().uv(24, 16)));
                }
                let mesh = registry
                    .mesh
                    .clone()
                    .expect("character mesh was initialized immediately above");
                let material = registry
                    .material
                    .get_or_insert_with(|| {
                        materials.add(StandardMaterial {
                            base_color: Color::srgb(0.9, 0.05, 0.05),
                            perceptual_roughness: 0.8,
                            unlit: true,
                            ..Default::default()
                        })
                    })
                    .clone();
                let character_id = snapshot.character_id;
                let entity = commands
                    .spawn((
                        Character { id: character_id },
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        transform,
                    ))
                    .observe(request_character_control)
                    .id();
                registry.entities.insert(character_id, entity);
            }
            ClientCharacterCommand::ControlGranted { character_id } => {
                if let Some(entity) = registry.entities.get(&character_id) {
                    commands.entity(*entity).insert(ControlledCharacter);
                }
            }
        }
    }
}

fn request_character_control(
    click: On<Pointer<Click>>,
    pipe: Res<ClientCharacterPipe>,
    characters: Query<&Character>,
) {
    if let Ok(character) = characters.get(click.entity) {
        let _ = pipe.0.try_send(ClientCharacterEvent::RequestControl {
            character_id: character.id,
        });
    }
}

fn snapshot_transform(snapshot: CharacterSnapshot) -> Transform {
    Transform {
        translation: Vec3::from_array(snapshot.translation),
        rotation: Quat::from_array(snapshot.rotation),
        scale: Vec3::from_array(snapshot.scale),
    }
}
