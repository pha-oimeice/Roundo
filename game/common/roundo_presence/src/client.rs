//! Client projection of authoritative presence snapshots into ECS marker entities.
//!
//! Each snapshot is treated as the complete desired player/world set. Missing
//! identities are despawned, existing identities retain their ECS entity, and
//! player translation is interpolated over one nominal server tick.

use crate::{JoinableWorld, JoinableWorldId, Player, PlayerId, PresenceSnapshot};
use bevy::prelude::{
    App, Commands, Component, DetectChanges, Entity, IntoScheduleConfigs, Name, Plugin, Quat,
    Query, Res, ResMut, Resource, Time, Transform, Update, Vec3,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
    LinearInterpolation,
};
use std::collections::{HashMap, HashSet};

/// Fallback visual radius for joinable-world marker entities.
pub const DEFAULT_JOINABLE_WORLD_RADIUS: f32 = 4.0;
const PLAYER_INTERPOLATION_DURATION_SECS: f32 = 1.0 / 20.0;

/// Clonable producer for authoritative presence commands.
///
/// Clones share one unbounded in-process queue; command submission is
/// non-blocking and does not wait for ECS application.
pub type ClientPresenceIpc = CrossbeamThreadPipeEndpointA<ClientPresenceCommand, ()>;

/// Installs presence-command projection and marker interpolation systems.
#[derive(Clone)]
pub struct RoundoPresenceClientPlugin {
    pipe: CrossbeamThreadPipe<ClientPresenceCommand, ()>,
}

impl RoundoPresenceClientPlugin {
    /// Creates a plugin with a fresh command queue.
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    /// Returns a producer sharing this plugin's command queue.
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

/// Client-side presentation settings for presence markers.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientPresenceSettings {
    joinable_world_radius: f32,
}

impl ClientPresenceSettings {
    /// Creates settings, replacing a non-finite or non-positive radius with the default.
    pub fn new(joinable_world_radius: f32) -> Self {
        Self {
            joinable_world_radius: valid_world_radius(joinable_world_radius),
        }
    }

    /// Returns the finite positive marker radius in local ECS units.
    pub fn joinable_world_radius(&self) -> f32 {
        self.joinable_world_radius
    }

    /// Replaces the radius after applying the same validation as [`Self::new`].
    ///
    /// Existing world-marker scales are synchronized during a subsequent
    /// [`Update`] schedule.
    pub fn set_joinable_world_radius(&mut self, radius: f32) {
        self.joinable_world_radius = valid_world_radius(radius);
    }
}

impl Default for ClientPresenceSettings {
    fn default() -> Self {
        Self::new(DEFAULT_JOINABLE_WORLD_RADIUS)
    }
}

/// Latest own-player identity received from the presence server.
///
/// The value is `None` before the first snapshot and after [`ClientPresenceCommand::Clear`].
#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalPlayerIdentity {
    player_id: Option<PlayerId>,
}

impl LocalPlayerIdentity {
    /// Returns the latest authoritative player identity, if connected.
    pub fn player_id(&self) -> Option<PlayerId> {
        self.player_id
    }
}

/// Authoritative replacement or teardown command consumed by the client ECS.
#[derive(Clone, Debug)]
pub enum ClientPresenceCommand {
    /// Reconciles markers to the complete supplied presence snapshot.
    Snapshot(PresenceSnapshot),
    /// Despawns all projected markers and clears the local player identity.
    Clear,
}

/// Domain-side player marker observed by rendering.
#[derive(Component, Clone, Copy, Debug)]
pub struct ClientPlayerMarker {
    /// Stable player identity represented by this ECS entity.
    pub player_id: PlayerId,
    /// Logical visibility requested from the rendering adapter.
    pub visible: bool,
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

/// Domain-side joinable-world marker observed by presentation systems.
#[derive(Component, Clone, Copy, Debug)]
pub struct ClientJoinableWorld {
    /// Stable world identity represented by this ECS entity.
    pub world_id: JoinableWorldId,
}

#[derive(Resource, Clone)]
struct ClientPresencePipe(CrossbeamThreadPipeEndpointB<ClientPresenceCommand, ()>);

#[derive(Clone, Copy)]
struct PresenceEntity {
    entity: Entity,
}

#[derive(Resource, Default)]
struct ClientPresenceRegistry {
    players: HashMap<PlayerId, PresenceEntity>,
    worlds: HashMap<JoinableWorldId, PresenceEntity>,
}

fn apply_presence_commands(
    mut commands: Commands,
    pipe: Res<ClientPresencePipe>,
    settings: Res<ClientPresenceSettings>,
    mut local_identity: ResMut<LocalPlayerIdentity>,
    mut registry: ResMut<ClientPresenceRegistry>,
    mut players: Query<&mut PlayerTranslationInterpolation>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientPresenceCommand::Snapshot(snapshot) => apply_snapshot(
                &mut commands,
                &settings,
                &mut registry,
                &mut players,
                &mut local_identity,
                snapshot,
            ),
            ClientPresenceCommand::Clear => {
                clear_presence(&mut commands, &mut registry);
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
        let entity = commands
            .spawn((
                Name::new(format!("Player {}", player_id.0)),
                Player { id: player_id },
                ClientPlayerMarker {
                    player_id,
                    visible: true,
                },
                PlayerTranslationInterpolation::stationary(player.translation),
                Transform::from_translation(Vec3::from_array(player.translation)),
            ))
            .id();
        registry
            .players
            .insert(player_id, PresenceEntity { entity });
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
        }
    }

    for world in snapshot.joinable_worlds {
        let transform = Transform::from_translation(Vec3::from_array(world.translation))
            .with_scale(Vec3::splat(settings.joinable_world_radius()));
        if let Some(visual) = registry.worlds.get(&world.world_id).copied() {
            commands.entity(visual.entity).insert((
                Name::new(world.name.clone()),
                JoinableWorld {
                    id: world.world_id,
                    name: world.name,
                },
                transform,
            ));
            continue;
        }
        let world_id = world.world_id;
        let entity = commands
            .spawn((
                Name::new(world.name.clone()),
                JoinableWorld {
                    id: world_id,
                    name: world.name,
                },
                ClientJoinableWorld { world_id },
                transform,
            ))
            .id();
        registry.worlds.insert(world_id, PresenceEntity { entity });
    }
}

fn clear_presence(commands: &mut Commands, registry: &mut ClientPresenceRegistry) {
    for visual in registry.players.drain().map(|(_, visual)| visual) {
        commands.entity(visual.entity).despawn();
    }
    for visual in registry.worlds.drain().map(|(_, visual)| visual) {
        commands.entity(visual.entity).despawn();
    }
}

fn animate_player_markers(
    time: Res<Time>,
    mut players: Query<(&mut PlayerTranslationInterpolation, &mut Transform)>,
) {
    for (mut interpolation, mut transform) in &mut players {
        transform.translation = Vec3::from_array(interpolation.0.advance(time.delta_secs()));
        transform.rotation *= Quat::from_rotation_y(time.delta_secs() * 1.4);
        transform.rotation *= Quat::from_rotation_x(time.delta_secs() * 0.7);
    }
}

fn sync_joinable_world_radius(
    settings: Res<ClientPresenceSettings>,
    mut worlds: Query<&mut Transform, bevy::prelude::With<ClientJoinableWorld>>,
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
    use super::*;
    use std::time::Duration;

    #[test]
    fn player_marker_translation_advances_between_network_points() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, animate_player_markers);
        let mut interpolation = PlayerTranslationInterpolation::stationary([2.0, 0.0, 0.0]);
        interpolation.0.retarget([2.0, 0.0, 0.0], [12.0, 0.0, 0.0]);
        let entity = app
            .world_mut()
            .spawn((
                ClientPlayerMarker {
                    player_id: PlayerId(1),
                    visible: true,
                },
                interpolation,
                Transform::from_xyz(2.0, 0.0, 0.0),
            ))
            .id();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(
                PLAYER_INTERPOLATION_DURATION_SECS / 2.0,
            ));
        app.update();

        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
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
