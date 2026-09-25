//! Authoritative test-Life bootstrap, Controller input, motion, and snapshot orchestration.

use crate::{
    creature_snapshot::{
        CommittedCreatureMotion, CreatureSnapshotMetadata, encode_creature_snapshot,
    },
    player_control::PlayerControlServerSet,
};
use bevy::prelude::{
    App, Component, FixedUpdate, IntoScheduleConfigs, Plugin, Query, Res, ResMut, Resource, Time,
    Transform, With, World,
};
use roundo_block_interaction::{BlockInteractionPose, RoutedBlockInteractionMessage};
use roundo_controller::{Controller, IntentDomain, register_controller};
use roundo_creature::{
    BodyOrientation, CreatureMotionPlugin, CreatureMotionSet, EnvironmentSample, ExecutionBodyId,
    GazeOrientation, IntentBodyId, KinematicVelocity, LifeLink, LocomotionState, TestLifePrototype,
};
use roundo_local_coordinate::{LocalCoordinateServerIpc, VirtualChunkEnvironmentMap};
use roundo_marionette::ServerMarionetteSnapshotSender;
use roundo_player::{Player, PlayerRegistry};

#[derive(Resource, Default)]
struct SimulationTickCounter(u64);

/// The two hard-coded, orthogonal Controller resources created for one test Life.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
struct TestLifeControllers {
    movement: roundo_contracts::ControllerId,
    gaze: roundo_contracts::ControllerId,
}

/// Idempotence marker for the development bootstrap owned by one Player.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
struct BootstrappedTestLife;

/// Owns the authoritative Creature lifecycle behind one server-composition interface.
///
/// Player, Controller, Creature Motion, Local Coordinate, and transport remain
/// independent modules. Their ordering and test-bootstrap policy are composed here.
pub(crate) struct AuthoritativeCreaturePlugin;

impl AuthoritativeCreaturePlugin {
    pub(crate) fn new(_local_coordinate: LocalCoordinateServerIpc) -> Self {
        Self
    }
}

impl Plugin for AuthoritativeCreaturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VirtualChunkEnvironmentMap>()
            .init_resource::<SimulationTickCounter>()
            .add_message::<RoutedBlockInteractionMessage>()
            .add_plugins(CreatureMotionPlugin)
            .configure_sets(
                FixedUpdate,
                PlayerControlServerSet::Commands.before(CreatureMotionSet::Commit),
            )
            .add_systems(
                FixedUpdate,
                sample_creature_environment.before(CreatureMotionSet::Commit),
            )
            .add_systems(
                FixedUpdate,
                publish_creature_snapshots.after(CreatureMotionSet::Commit),
            );
    }
}

/// Ensures one non-spatial Player owns exactly one development test Life.
///
/// The operation is synchronous so a same-tick Access request cannot observe a
/// newly created Player before its grants exist. It grants Access only; neither
/// Controller is acquired here.
pub(crate) fn ensure_test_life_for_player(
    world: &mut World,
    player_id: roundo_contracts::PlayerId,
) {
    let player_entity = world
        .resource::<PlayerRegistry>()
        .player_entity(player_id)
        .expect("resolved Player has an authoritative ECS entity");
    if world.get::<BootstrappedTestLife>(player_entity).is_some() {
        return;
    }

    let (_, members) = TestLifePrototype::default().instantiate(
        world,
        bevy::prelude::Vec3::ZERO,
        EnvironmentSample::default(),
    );
    world
        .entity_mut(members.execution)
        .insert(BlockInteractionPose);
    let movement = register_controller(world, members.intent, IntentDomain::movement())
        .expect("fresh test Life accepts its movement Controller");
    let gaze = register_controller(world, members.intent, IntentDomain::gaze())
        .expect("fresh test Life accepts its gaze Controller");
    let controllers = TestLifeControllers { movement, gaze };
    world.entity_mut(members.execution).insert(controllers);
    let mut player = world
        .get_mut::<Player>(player_entity)
        .expect("resolved Player remains live while bootstrapping");
    player.set_controller_access(movement, roundo_contracts::ControllerAccessLevel::ReadWrite);
    player.set_controller_access(gaze, roundo_contracts::ControllerAccessLevel::ReadWrite);
    world.entity_mut(player_entity).insert(BootstrappedTestLife);
}

fn sample_creature_environment(
    environment: Res<VirtualChunkEnvironmentMap>,
    mut creatures: Query<(&Transform, &mut EnvironmentSample), With<ExecutionBodyId>>,
) {
    for (transform, mut sample) in &mut creatures {
        if let Some((values, revision)) = environment.sample(transform.translation) {
            *sample = EnvironmentSample::new(
                values.gravity,
                values.static_friction,
                values.kinetic_friction,
                revision,
            )
            .expect("environment map publishes validated values");
        }
    }
}

fn publish_creature_snapshots(
    sender: Res<ServerMarionetteSnapshotSender>,
    sequences: Res<crate::player_control::ControllerInputSequences>,
    mut tick: ResMut<SimulationTickCounter>,
    fixed_time: Res<Time<bevy::prelude::Fixed>>,
    controllers: Query<&Controller>,
    players: Res<PlayerRegistry>,
    intent_bodies: Query<&LifeLink, With<IntentBodyId>>,
    executions: Query<
        (
            &Transform,
            &BodyOrientation,
            &GazeOrientation,
            &KinematicVelocity,
            &LocomotionState,
            &EnvironmentSample,
            &TestLifeControllers,
        ),
        With<ExecutionBodyId>,
    >,
) {
    use std::collections::HashSet;
    tick.0 = tick.0.wrapping_add(1);
    let mut sent = HashSet::new();
    for controller in &controllers {
        let Some(player_id) = controller.current_player() else {
            continue;
        };
        let Some(link) = intent_bodies.get(controller.intent_body()).ok() else {
            continue;
        };
        let Ok((transform, body, gaze, velocity, locomotion, environment, controller_ids)) =
            executions.get(link.entities().execution)
        else {
            continue;
        };
        let snapshot = encode_creature_snapshot(
            CommittedCreatureMotion {
                transform,
                body,
                gaze,
                velocity,
                locomotion,
                environment,
            },
            CreatureSnapshotMetadata {
                fixed_dt_seconds: fixed_time.delta_secs(),
                simulation_tick: tick.0,
                acknowledged_movement: sequences.movement_sequence(controller_ids.movement),
                acknowledged_gaze: sequences.gaze_sequence(controller_ids.gaze),
            },
        );
        for connection_id in players.connections_for_player(player_id) {
            // Two orthogonal Controllers of one Life yield exactly one snapshot per client/tick.
            if sent.insert((connection_id, link.id())) {
                let _ = sender.0.try_send(
                    roundo_marionette::ServerMarionetteEvent::CreatureMotionSnapshot {
                        connection_id,
                        snapshot,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roundo_controller::controller_state;
    use roundo_player::{ExternalIdentityKey, find_or_create_player};

    #[test]
    fn bootstrap_creates_two_orthogonal_uncontrolled_controllers_and_grants_access() {
        let mut app = App::new();
        app.add_plugins(CreatureMotionPlugin);
        let player_id = find_or_create_player(
            app.world_mut(),
            ExternalIdentityKey::new("peer:test").unwrap(),
        );
        ensure_test_life_for_player(app.world_mut(), player_id);
        ensure_test_life_for_player(app.world_mut(), player_id);
        let player_entity = app
            .world()
            .resource::<PlayerRegistry>()
            .player_entity(player_id)
            .unwrap();
        let player = app.world().get::<Player>(player_entity).unwrap();
        assert!(app.world().get::<Transform>(player_entity).is_none());
        let access = player.controller_accesses().collect::<Vec<_>>();
        assert_eq!(access.len(), 2);
        assert_eq!(
            app.world_mut()
                .query::<&IntentBodyId>()
                .iter(app.world())
                .count(),
            1,
            "idempotent bootstrap must not create an orphan second Life"
        );
        for (controller_id, level) in access {
            assert_eq!(level, roundo_contracts::ControllerAccessLevel::ReadWrite);
            let (domain, current) = controller_state(app.world(), controller_id).unwrap();
            assert_eq!(current, None);
            assert!(domain == IntentDomain::movement() || domain == IntentDomain::gaze());
        }
    }
}
