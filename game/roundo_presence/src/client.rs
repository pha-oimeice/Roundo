use crate::{JoinableWorld, JoinableWorldId, Player, PlayerId, PresenceSnapshot};
use bevy::{
    mesh::Meshable,
    prelude::{
        App, Assets, Camera, Camera3d, Color, Commands, Component, DetectChanges, GlobalTransform,
        Mesh, Mesh3d, MeshMaterial3d, Name, Plugin, Query, Res, ResMut, Resource, Sphere,
        StandardMaterial, Tetrahedron, Time, Transform, Update, Vec3, With,
    },
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{HashMap, HashSet};

pub const DEFAULT_JOINABLE_WORLD_RADIUS: f32 = 4.0;

pub type ClientPresenceIpc =
    CrossbeamThreadPipeEndpointA<ClientPresenceCommand, ClientPresenceEvent>;

#[derive(Clone)]
pub struct RoundoPresenceClientPlugin {
    pipe: CrossbeamThreadPipe<ClientPresenceCommand, ClientPresenceEvent>,
}

impl RoundoPresenceClientPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> ClientPresenceIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for RoundoPresenceClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for RoundoPresenceClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientPresenceSettings>()
            .init_resource::<LocalPlayerIdentity>()
            .init_resource::<ClientPresenceRegistry>()
            .init_resource::<LastPublishedPosition>()
            .insert_resource(ClientPresencePipe(self.pipe.endpoint_b()))
            .add_systems(
                Update,
                (
                    apply_presence_commands,
                    publish_local_player_position,
                    rotate_player_markers,
                    sync_joinable_world_radius,
                ),
            );
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientPresenceSettings {
    joinable_world_radius: f32,
}

impl ClientPresenceSettings {
    pub fn new(joinable_world_radius: f32) -> Self {
        Self {
            joinable_world_radius: valid_world_radius(joinable_world_radius),
        }
    }

    pub fn joinable_world_radius(&self) -> f32 {
        self.joinable_world_radius
    }

    pub fn set_joinable_world_radius(&mut self, radius: f32) {
        self.joinable_world_radius = valid_world_radius(radius);
    }
}

impl Default for ClientPresenceSettings {
    fn default() -> Self {
        Self::new(DEFAULT_JOINABLE_WORLD_RADIUS)
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalPlayerIdentity {
    player_id: Option<PlayerId>,
}

impl LocalPlayerIdentity {
    pub fn player_id(&self) -> Option<PlayerId> {
        self.player_id
    }
}

#[derive(Clone, Debug)]
pub enum ClientPresenceCommand {
    Snapshot(PresenceSnapshot),
    Clear,
}

#[derive(Clone, Debug)]
pub enum ClientPresenceEvent {
    PositionChanged { translation: [f32; 3] },
}

#[derive(Component, Clone, Copy, Debug)]
pub struct ClientPlayerMarker {
    pub player_id: PlayerId,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct ClientJoinableWorld {
    pub world_id: JoinableWorldId,
}

#[derive(Resource, Clone)]
struct ClientPresencePipe(CrossbeamThreadPipeEndpointB<ClientPresenceCommand, ClientPresenceEvent>);

#[derive(Resource, Default)]
struct ClientPresenceRegistry {
    players: HashMap<PlayerId, bevy::prelude::Entity>,
    worlds: HashMap<JoinableWorldId, bevy::prelude::Entity>,
    player_mesh: Option<bevy::prelude::Handle<Mesh>>,
    player_material: Option<bevy::prelude::Handle<StandardMaterial>>,
    world_mesh: Option<bevy::prelude::Handle<Mesh>>,
    world_material: Option<bevy::prelude::Handle<StandardMaterial>>,
}

#[derive(Resource, Default)]
struct LastPublishedPosition(Option<Vec3>);

fn apply_presence_commands(
    mut commands: Commands,
    pipe: Res<ClientPresencePipe>,
    settings: Res<ClientPresenceSettings>,
    mut local_identity: ResMut<LocalPlayerIdentity>,
    mut registry: ResMut<ClientPresenceRegistry>,
    mut last_position: ResMut<LastPublishedPosition>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientPresenceCommand::Snapshot(snapshot) => apply_snapshot(
                &mut commands,
                &settings,
                &mut registry,
                &mut transforms,
                &mut meshes,
                &mut materials,
                &mut local_identity,
                snapshot,
            ),
            ClientPresenceCommand::Clear => {
                clear_presence(&mut commands, &mut registry);
                last_position.0 = None;
                local_identity.player_id = None;
            }
        }
    }
}

fn apply_snapshot(
    commands: &mut Commands,
    settings: &ClientPresenceSettings,
    registry: &mut ClientPresenceRegistry,
    transforms: &mut Query<&mut Transform>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    local_identity: &mut LocalPlayerIdentity,
    snapshot: PresenceSnapshot,
) {
    local_identity.player_id = Some(snapshot.own_player_id);
    let desired_players = snapshot
        .players
        .iter()
        .map(|player| player.player_id)
        .collect::<HashSet<_>>();
    for player_id in registry.players.keys().copied().collect::<Vec<_>>() {
        if !desired_players.contains(&player_id)
            && let Some(entity) = registry.players.remove(&player_id)
        {
            commands.entity(entity).despawn();
        }
    }

    for player in snapshot.players {
        if let Some(entity) = registry.players.get(&player.player_id).copied() {
            if let Ok(mut transform) = transforms.get_mut(entity) {
                transform.translation = Vec3::from_array(player.translation);
            }
            continue;
        }
        let mesh = registry
            .player_mesh
            .get_or_insert_with(|| meshes.add(Mesh::from(Tetrahedron::default())))
            .clone();
        let material = registry
            .player_material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.55, 0.05),
                    perceptual_roughness: 0.55,
                    unlit: true,
                    ..Default::default()
                })
            })
            .clone();
        let player_id = player.player_id;
        let entity = commands
            .spawn((
                Name::new(format!("Player {}", player_id.0)),
                Player { id: player_id },
                ClientPlayerMarker { player_id },
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(Vec3::from_array(player.translation)),
            ))
            .id();
        registry.players.insert(player_id, entity);
    }

    let desired_worlds = snapshot
        .joinable_worlds
        .iter()
        .map(|world| world.world_id)
        .collect::<HashSet<_>>();
    for world_id in registry.worlds.keys().copied().collect::<Vec<_>>() {
        if !desired_worlds.contains(&world_id)
            && let Some(entity) = registry.worlds.remove(&world_id)
        {
            commands.entity(entity).despawn();
        }
    }

    for world in snapshot.joinable_worlds {
        if let Some(entity) = registry.worlds.get(&world.world_id).copied() {
            if let Ok(mut transform) = transforms.get_mut(entity) {
                transform.translation = Vec3::from_array(world.translation);
            }
            commands.entity(entity).insert((
                Name::new(world.name.clone()),
                JoinableWorld {
                    id: world.world_id,
                    name: world.name,
                },
            ));
            continue;
        }
        let mesh = registry
            .world_mesh
            .get_or_insert_with(|| meshes.add(Sphere::new(1.0).mesh().uv(32, 18)))
            .clone();
        let material = registry
            .world_material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::srgb(0.12, 0.35, 0.95),
                    perceptual_roughness: 0.7,
                    metallic: 0.1,
                    unlit: true,
                    ..Default::default()
                })
            })
            .clone();
        let world_id = world.world_id;
        let entity = commands
            .spawn((
                Name::new(world.name.clone()),
                JoinableWorld {
                    id: world_id,
                    name: world.name,
                },
                ClientJoinableWorld { world_id },
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(Vec3::from_array(world.translation))
                    .with_scale(Vec3::splat(settings.joinable_world_radius())),
            ))
            .id();
        registry.worlds.insert(world_id, entity);
    }
}

fn clear_presence(commands: &mut Commands, registry: &mut ClientPresenceRegistry) {
    for entity in registry.players.drain().map(|(_, entity)| entity) {
        commands.entity(entity).despawn();
    }
    for entity in registry.worlds.drain().map(|(_, entity)| entity) {
        commands.entity(entity).despawn();
    }
}

fn publish_local_player_position(
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    pipe: Res<ClientPresencePipe>,
    mut last_position: ResMut<LastPublishedPosition>,
) {
    let Some(position) = cameras
        .iter()
        .find_map(|(camera, transform)| camera.is_active.then_some(transform.translation()))
    else {
        return;
    };
    if last_position
        .0
        .is_some_and(|last| last.distance_squared(position) <= 0.0001)
    {
        return;
    }
    last_position.0 = Some(position);
    let _ = pipe.0.try_send(ClientPresenceEvent::PositionChanged {
        translation: position.to_array(),
    });
}

fn rotate_player_markers(
    time: Res<Time>,
    mut players: Query<&mut Transform, With<ClientPlayerMarker>>,
) {
    for mut transform in &mut players {
        transform.rotate_y(time.delta_secs() * 1.4);
        transform.rotate_x(time.delta_secs() * 0.7);
    }
}

fn sync_joinable_world_radius(
    settings: Res<ClientPresenceSettings>,
    mut worlds: Query<&mut Transform, With<ClientJoinableWorld>>,
) {
    if !settings.is_changed() {
        return;
    }
    for mut transform in &mut worlds {
        transform.scale = Vec3::splat(settings.joinable_world_radius());
    }
}

fn valid_world_radius(radius: f32) -> f32 {
    if radius.is_finite() && radius > 0.0 {
        radius
    } else {
        DEFAULT_JOINABLE_WORLD_RADIUS
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientPresenceSettings, DEFAULT_JOINABLE_WORLD_RADIUS};

    #[test]
    fn invalid_world_radius_uses_default() {
        assert_eq!(
            ClientPresenceSettings::new(f32::INFINITY).joinable_world_radius(),
            DEFAULT_JOINABLE_WORLD_RADIUS
        );
    }
}
