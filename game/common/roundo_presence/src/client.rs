//! Client projection of authoritative presence snapshots into ECS marker entities.
//!
//! Each snapshot is treated as the complete desired player/world set. Missing
//! identities are despawned, existing identities retain their ECS entity, player
//! translation is interpolated over one nominal server tick, and authoritative
//! gaze rotation is projected without synthetic animation.

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
            .init_resource::<ClientPresenceSession>()
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
/// The value is `None` outside an active session and before its first snapshot.
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

/// Authoritative session lifecycle and replacement snapshots consumed by client ECS.
#[derive(Clone, Debug)]
pub enum ClientPresenceCommand {
    /// Starts a fresh authority session after invalidating every prior projection.
    BeginSession { epoch: u64 },
    /// Reconciles markers to the complete supplied presence snapshot.
    Snapshot(PresenceSnapshot),
    /// Ends the matching authority session. Repeated or stale ends are harmless.
    EndSession { epoch: u64 },
}

#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClientPresenceSession {
    epoch: Option<u64>,
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
    mut session: ResMut<ClientPresenceSession>,
    mut local_identity: ResMut<LocalPlayerIdentity>,
    mut registry: ResMut<ClientPresenceRegistry>,
    mut players: Query<(&mut PlayerTranslationInterpolation, &mut Transform)>,
) {
    while let Some(command) = pipe.0.try_receive() {
        match command {
            ClientPresenceCommand::BeginSession { epoch } => {
                clear_presence(&mut commands, &mut registry);
                local_identity.player_id = None;
                session.epoch = Some(epoch);
            }
            ClientPresenceCommand::Snapshot(snapshot) if session.epoch.is_some() => apply_snapshot(
                &mut commands,
                &settings,
                &mut registry,
                &mut players,
                &mut local_identity,
                snapshot,
            ),
            ClientPresenceCommand::Snapshot(_) => {}
            ClientPresenceCommand::EndSession { epoch } if session.epoch == Some(epoch) => {
                clear_presence(&mut commands, &mut registry);
                local_identity.player_id = None;
                session.epoch = None;
            }
            ClientPresenceCommand::EndSession { .. } => {}
        }
    }
}

fn apply_snapshot(
    commands: &mut Commands,
    settings: &ClientPresenceSettings,
    registry: &mut ClientPresenceRegistry,
    players: &mut Query<(&mut PlayerTranslationInterpolation, &mut Transform)>,
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
            if let Ok((mut interpolation, mut transform)) = players.get_mut(visual.entity) {
                let current = interpolation.0.value();
                interpolation.0.retarget(current, player.translation);
                transform.rotation = validated_rotation(player.rotation);
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
                Transform::from_translation(Vec3::from_array(player.translation))
                    .with_rotation(validated_rotation(player.rotation)),
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

fn validated_rotation(rotation: [f32; 4]) -> Quat {
    let rotation = Quat::from_array(rotation);
    if rotation.is_finite() && rotation.length_squared() > f32::EPSILON {
        rotation.normalize()
    } else {
        Quat::IDENTITY
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
        let original_rotation = Quat::from_rotation_y(0.7);
        let entity = app
            .world_mut()
            .spawn((
                ClientPlayerMarker {
                    player_id: PlayerId(1),
                    visible: true,
                },
                interpolation,
                Transform::from_xyz(2.0, 0.0, 0.0).with_rotation(original_rotation),
            ))
            .id();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(
                PLAYER_INTERPOLATION_DURATION_SECS / 2.0,
            ));
        app.update();

        let transform = app.world().get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation, Vec3::new(7.0, 0.0, 0.0));
        assert!(
            transform
                .rotation
                .abs_diff_eq(original_rotation, f32::EPSILON)
        );
    }

    #[test]
    fn authoritative_player_rotation_updates_the_projected_gaze() {
        let plugin = RoundoPresenceClientPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time>().add_plugins(plugin);
        let player_id = PlayerId(3);
        let expected = Quat::from_rotation_y(1.1);
        ipc.try_send(ClientPresenceCommand::BeginSession { epoch: 1 })
            .unwrap();
        ipc.try_send(ClientPresenceCommand::Snapshot(PresenceSnapshot {
            own_player_id: player_id,
            players: vec![roundo_contracts::NearbyPlayer {
                player_id,
                translation: [0.0; 3],
                rotation: expected.to_array(),
            }],
            joinable_worlds: Vec::new(),
        }))
        .unwrap();

        app.update();

        let transform = app
            .world_mut()
            .query_filtered::<&Transform, bevy::prelude::With<ClientPlayerMarker>>()
            .single(app.world())
            .unwrap();
        assert!(transform.rotation.abs_diff_eq(expected, f32::EPSILON));
    }

    #[test]
    fn ending_a_session_removes_identity_and_every_presence_projection() {
        let plugin = RoundoPresenceClientPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time>().add_plugins(plugin);
        ipc.try_send(ClientPresenceCommand::BeginSession { epoch: 3 })
            .unwrap();
        ipc.try_send(ClientPresenceCommand::Snapshot(PresenceSnapshot {
            own_player_id: PlayerId(8),
            players: vec![roundo_contracts::NearbyPlayer {
                player_id: PlayerId(8),
                translation: [1.0, 2.0, 3.0],
                rotation: Quat::IDENTITY.to_array(),
            }],
            joinable_worlds: Vec::new(),
        }))
        .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<LocalPlayerIdentity>().player_id(),
            Some(PlayerId(8))
        );

        ipc.try_send(ClientPresenceCommand::EndSession { epoch: 3 })
            .unwrap();
        app.update();

        assert_eq!(
            app.world().resource::<LocalPlayerIdentity>().player_id(),
            None
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, bevy::prelude::With<ClientPlayerMarker>>()
                .iter(app.world())
                .count(),
            0
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
