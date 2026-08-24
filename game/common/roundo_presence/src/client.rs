use crate::{JoinableWorld, JoinableWorldId, Player, PlayerId, PresenceSnapshot};
use bevy::prelude::{
    App, Commands, Component, DetectChanges, IntoScheduleConfigs, Plugin, Quat, Query, Res, ResMut,
    Resource, Time, Update, Vec3,
};
use roundo_rendering::{
    RenderMaterial, RenderMesh, RenderObject, RenderObjectId, RenderObjects, RenderTransform,
    issue_render_object, remove_render_object, render_object_transform, update_render_object_name,
    update_render_object_transform,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
    LinearInterpolation,
};
use std::collections::{HashMap, HashSet};

pub const DEFAULT_JOINABLE_WORLD_RADIUS: f32 = 4.0;
const PLAYER_INTERPOLATION_DURATION_SECS: f32 = 1.0 / 20.0;

pub type ClientPresenceIpc = CrossbeamThreadPipeEndpointA<ClientPresenceCommand, ()>;

#[derive(Clone)]
pub struct RoundoPresenceClientPlugin {
    pipe: CrossbeamThreadPipe<ClientPresenceCommand, ()>,
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
            .insert_resource(ClientPresencePipe(self.pipe.endpoint_b()))
            .add_systems(
                Update,
                (
                    animate_player_markers,
                    apply_presence_commands,
                    sync_joinable_world_radius,
                )
                    .chain(),
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

#[derive(Component, Clone, Copy, Debug)]
pub struct ClientPlayerMarker {
    pub player_id: PlayerId,
    pub render_object_id: RenderObjectId,
}

#[derive(Component, Clone, Copy, Debug)]
struct PlayerTranslationInterpolation(LinearInterpolation<3>);

impl PlayerTranslationInterpolation {
    fn stationary(translation: [f32; 3]) -> Self {
        Self(LinearInterpolation::stationary(
            translation,
            PLAYER_INTERPOLATION_DURATION_SECS,
        ))
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct ClientJoinableWorld {
    pub world_id: JoinableWorldId,
    pub render_object_id: RenderObjectId,
}

#[derive(Resource, Clone)]
struct ClientPresencePipe(CrossbeamThreadPipeEndpointB<ClientPresenceCommand, ()>);

#[derive(Clone, Copy)]
struct PresenceVisual {
    entity: bevy::prelude::Entity,
    render_object_id: RenderObjectId,
}

#[derive(Resource, Default)]
struct ClientPresenceRegistry {
    players: HashMap<PlayerId, PresenceVisual>,
    worlds: HashMap<JoinableWorldId, PresenceVisual>,
}

fn apply_presence_commands(
    mut commands: Commands,
    pipe: Res<ClientPresencePipe>,
    settings: Res<ClientPresenceSettings>,
    mut local_identity: ResMut<LocalPlayerIdentity>,
    mut registry: ResMut<ClientPresenceRegistry>,
    mut players: Query<&mut PlayerTranslationInterpolation>,
    mut render_objects: ResMut<RenderObjects>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientPresenceCommand::Snapshot(snapshot) => apply_snapshot(
                &mut commands,
                &settings,
                &mut registry,
                &mut players,
                &mut render_objects,
                &mut local_identity,
                snapshot,
            ),
            ClientPresenceCommand::Clear => {
                clear_presence(&mut commands, &mut registry, &mut render_objects);
                local_identity.player_id = None;
            }
        }
    }
}

fn apply_snapshot(
    commands: &mut Commands,
    settings: &ClientPresenceSettings,
    registry: &mut ClientPresenceRegistry,
    players: &mut Query<&mut PlayerTranslationInterpolation>,
    render_objects: &mut RenderObjects,
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
            && let Some(visual) = registry.players.remove(&player_id)
        {
            commands.entity(visual.entity).despawn();
            remove_render_object(render_objects, visual.render_object_id);
        }
    }

    for player in snapshot.players {
        if let Some(visual) = registry.players.get(&player.player_id).copied() {
            if let Ok(mut interpolation) = players.get_mut(visual.entity) {
                let current = interpolation.0.value();
                interpolation.0.retarget(current, player.translation);
            }
            continue;
        }
        let player_id = player.player_id;
        let render_object_id = issue_render_object(
            render_objects,
            RenderObject::new(
                RenderMesh::tetrahedron(),
                RenderMaterial::unlit([1.0, 0.55, 0.05, 1.0]).with_perceptual_roughness(0.55),
                RenderTransform::from_translation(Vec3::from_array(player.translation)),
            )
            .with_name(format!("Player {}", player_id.0)),
        );
        let entity = commands
            .spawn((
                Player { id: player_id },
                ClientPlayerMarker {
                    player_id,
                    render_object_id,
                },
                PlayerTranslationInterpolation::stationary(player.translation),
            ))
            .id();
        registry.players.insert(
            player_id,
            PresenceVisual {
                entity,
                render_object_id,
            },
        );
    }

    let desired_worlds = snapshot
        .joinable_worlds
        .iter()
        .map(|world| world.world_id)
        .collect::<HashSet<_>>();
    for world_id in registry.worlds.keys().copied().collect::<Vec<_>>() {
        if !desired_worlds.contains(&world_id)
            && let Some(visual) = registry.worlds.remove(&world_id)
        {
            commands.entity(visual.entity).despawn();
            remove_render_object(render_objects, visual.render_object_id);
        }
    }

    for world in snapshot.joinable_worlds {
        if let Some(visual) = registry.worlds.get(&world.world_id).copied() {
            update_render_object_transform(
                render_objects,
                visual.render_object_id,
                RenderTransform::from_translation(Vec3::from_array(world.translation))
                    .with_scale(Vec3::splat(settings.joinable_world_radius())),
            );
            update_render_object_name(render_objects, visual.render_object_id, world.name.clone());
            commands.entity(visual.entity).insert(JoinableWorld {
                id: world.world_id,
                name: world.name,
            });
            continue;
        }
        let world_id = world.world_id;
        let render_object_id = issue_render_object(
            render_objects,
            RenderObject::new(
                RenderMesh::uv_sphere(1.0, 32, 18),
                RenderMaterial::unlit([0.12, 0.35, 0.95, 1.0])
                    .with_perceptual_roughness(0.7)
                    .with_metallic(0.1),
                RenderTransform::from_translation(Vec3::from_array(world.translation))
                    .with_scale(Vec3::splat(settings.joinable_world_radius())),
            )
            .with_name(world.name.clone()),
        );
        let entity = commands
            .spawn((
                JoinableWorld {
                    id: world_id,
                    name: world.name,
                },
                ClientJoinableWorld {
                    world_id,
                    render_object_id,
                },
            ))
            .id();
        registry.worlds.insert(
            world_id,
            PresenceVisual {
                entity,
                render_object_id,
            },
        );
    }
}

fn clear_presence(
    commands: &mut Commands,
    registry: &mut ClientPresenceRegistry,
    render_objects: &mut RenderObjects,
) {
    for visual in registry.players.drain().map(|(_, visual)| visual) {
        commands.entity(visual.entity).despawn();
        remove_render_object(render_objects, visual.render_object_id);
    }
    for visual in registry.worlds.drain().map(|(_, visual)| visual) {
        commands.entity(visual.entity).despawn();
        remove_render_object(render_objects, visual.render_object_id);
    }
}

fn animate_player_markers(
    time: Res<Time>,
    mut players: Query<(&ClientPlayerMarker, &mut PlayerTranslationInterpolation)>,
    mut render_objects: ResMut<RenderObjects>,
) {
    for (player, mut interpolation) in &mut players {
        let Some(mut transform) = render_object_transform(&render_objects, player.render_object_id)
        else {
            continue;
        };
        transform.translation = Vec3::from_array(interpolation.0.advance(time.delta_secs()));
        transform.rotation *= Quat::from_rotation_y(time.delta_secs() * 1.4);
        transform.rotation *= Quat::from_rotation_x(time.delta_secs() * 0.7);
        update_render_object_transform(&mut render_objects, player.render_object_id, transform);
    }
}

fn sync_joinable_world_radius(
    settings: Res<ClientPresenceSettings>,
    worlds: Query<&ClientJoinableWorld>,
    mut render_objects: ResMut<RenderObjects>,
) {
    if !settings.is_changed() {
        return;
    }
    for world in &worlds {
        let Some(mut transform) = render_object_transform(&render_objects, world.render_object_id)
        else {
            continue;
        };
        transform.scale = Vec3::splat(settings.joinable_world_radius());
        update_render_object_transform(&mut render_objects, world.render_object_id, transform);
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
    use super::*;
    use std::time::Duration;

    #[test]
    fn player_marker_translation_advances_between_network_points() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<RenderObjects>()
            .add_systems(Update, animate_player_markers);
        let render_object_id = {
            let mut render_objects = app.world_mut().resource_mut::<RenderObjects>();
            issue_render_object(
                &mut render_objects,
                RenderObject::new(
                    RenderMesh::tetrahedron(),
                    RenderMaterial::unlit([1.0; 4]),
                    RenderTransform::from_translation(Vec3::new(2.0, 0.0, 0.0)),
                ),
            )
        };
        let mut interpolation = PlayerTranslationInterpolation::stationary([2.0, 0.0, 0.0]);
        interpolation.0.retarget([2.0, 0.0, 0.0], [12.0, 0.0, 0.0]);
        app.world_mut().spawn((
            ClientPlayerMarker {
                player_id: PlayerId(1),
                render_object_id,
            },
            interpolation,
        ));

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(
                PLAYER_INTERPOLATION_DURATION_SECS / 2.0,
            ));
        app.update();

        assert_eq!(
            render_object_transform(app.world().resource(), render_object_id)
                .unwrap()
                .translation,
            Vec3::new(7.0, 0.0, 0.0)
        );
    }

    #[test]
    fn invalid_world_radius_uses_default() {
        assert_eq!(
            ClientPresenceSettings::new(f32::INFINITY).joinable_world_radius(),
            DEFAULT_JOINABLE_WORLD_RADIUS
        );
    }
}
