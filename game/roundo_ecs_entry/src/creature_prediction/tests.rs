use super::*;
use bevy::ecs::system::RunSystemOnce;
use roundo_contracts::{
    CreatureEnvironmentSample, CreatureLocomotion, EnvironmentRevision, InputSequence,
    SimulationTick,
};
use roundo_creature::{GazeAction, MovementAction};

fn snapshot(tick: u64, pos: [f32; 3]) -> roundo_contracts::CreatureMotionSnapshot {
    roundo_contracts::CreatureMotionSnapshot {
        fixed_dt_seconds: 1.0 / 64.0,
        simulation_tick: SimulationTick(tick),
        translation: pos,
        body_orientation: [0.0, 0.0, 0.0, 1.0],
        gaze_orientation: [0.0, 0.0, 0.0, 1.0],
        velocity: [0.0; 3],
        locomotion: CreatureLocomotion::Floating,
        environment: CreatureEnvironmentSample {
            gravity: [0.0, -9.81, 0.0],
            static_friction: 0.6,
            kinetic_friction: 0.5,
            revision: EnvironmentRevision(0),
        },
        acknowledged_movement: InputSequence(0),
        acknowledged_gaze: InputSequence(0),
    }
}

fn set_authority(
    world: &mut World,
    epoch: Option<u64>,
    snapshot: Option<roundo_contracts::CreatureMotionSnapshot>,
) {
    let mut authority = world.resource_mut::<PendingAuthoritySnapshot>();
    authority.session_epoch = epoch;
    authority.snapshot = snapshot;
}

fn journal_world() -> World {
    let mut world = World::new();
    world.init_resource::<PredictionRuntime>();
    world.init_resource::<PredictionInputs>();
    let entity = world
        .spawn((
            PredictedCreature,
            MovementAction::default(),
            GazeAction::default(),
        ))
        .id();
    world.resource_mut::<PredictionRuntime>().entity = Some(entity);
    world
}

#[test]
fn fixed_ticks_journal_effective_intents() {
    let mut world = journal_world();
    let mut inputs = world.resource_mut::<PredictionInputs>();
    inputs.record_movement(3, [1.0, 0.0, 0.0]);
    inputs.record_gaze(5, 0.2, -0.1);
    drop(inputs);

    world
        .run_system_once(apply_prediction_journal_tick)
        .unwrap();
    world
        .run_system_once(apply_prediction_journal_tick)
        .unwrap();

    let inputs = world.resource::<PredictionInputs>();
    assert_eq!(inputs.journal_len(), 2);
    let first = inputs.journal_front().unwrap();
    assert_eq!(first.movement, Some((3, [1.0, 0.0, 0.0])));
    assert_eq!(first.gaze, Some((5, 0.2, -0.1)));
}

#[test]
fn ack_and_old_snapshot_are_handled_without_rollback() {
    let mut world = World::new();
    world.init_resource::<PredictionRuntime>();
    world.init_resource::<PredictionInputs>();
    world.init_resource::<PendingAuthoritySnapshot>();
    world.init_resource::<CameraTranslationProjection>();
    world.init_resource::<Time<Fixed>>();
    world.resource_mut::<PredictionRuntime>().session_epoch = Some(1);
    {
        let mut inputs = world.resource_mut::<PredictionInputs>();
        inputs.record_movement(2, [1.0; 3]);
        inputs.record_movement(4, [0.0; 3]);
    }
    set_authority(
        &mut world,
        Some(1),
        Some(roundo_contracts::CreatureMotionSnapshot {
            acknowledged_movement: InputSequence(2),
            ..snapshot(8, [8.0; 3])
        }),
    );
    restore_authoritative_creature(&mut world);
    assert_eq!(
        world.resource::<PredictionInputs>().pending_movement_len(),
        1
    );

    set_authority(&mut world, Some(1), Some(snapshot(7, [7.0; 3])));
    restore_authoritative_creature(&mut world);
    let entity = world.resource::<PredictionRuntime>().entity.unwrap();
    assert_eq!(
        world.get::<Transform>(entity).unwrap().translation,
        Vec3::splat(8.0)
    );
}

#[derive(Resource, Default)]
struct UnrelatedFixedRuns(u32);

fn count_unrelated_fixed_runs(mut runs: ResMut<UnrelatedFixedRuns>) {
    runs.0 += 1;
}

#[test]
fn authoritative_ack_replays_only_creature_motion() {
    let mut app = App::new();
    app.init_resource::<Time<Fixed>>()
        .init_resource::<UnrelatedFixedRuns>()
        .add_plugins((avian3d::PhysicsPlugins::default(), CreaturePredictionPlugin))
        .add_systems(FixedUpdate, count_unrelated_fixed_runs);
    set_authority(app.world_mut(), Some(1), Some(snapshot(1, [0.0; 3])));
    restore_authoritative_creature(app.world_mut());
    app.world_mut()
        .resource_mut::<PredictionInputs>()
        .record_movement(1, [1.0, 0.0, 0.0]);
    for _ in 0..2 {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(Duration::from_secs_f32(1.0 / 64.0));
        app.world_mut().run_schedule(FixedUpdate);
    }
    assert_eq!(app.world().resource::<PredictionInputs>().journal_len(), 2);
    let unrelated_before_replay = app.world().resource::<UnrelatedFixedRuns>().0;

    set_authority(
        app.world_mut(),
        Some(1),
        Some(roundo_contracts::CreatureMotionSnapshot {
            acknowledged_movement: InputSequence(1),
            ..snapshot(2, [0.0; 3])
        }),
    );
    restore_authoritative_creature(app.world_mut());

    assert_eq!(
        app.world().resource::<PredictionRuntime>().predicted_tick,
        Some(3)
    );
    let inputs = app.world().resource::<PredictionInputs>();
    assert_eq!(inputs.journal_len(), 1);
    assert_eq!(inputs.journal_front().unwrap().tick, 3);
    assert_eq!(
        app.world().resource::<UnrelatedFixedRuns>().0,
        unrelated_before_replay,
        "replay must not re-enter unrelated FixedUpdate systems"
    );
    let creature = app.world().resource::<PredictionRuntime>().entity.unwrap();
    let pose = app.world().get::<Transform>(creature).unwrap();
    assert!(pose.translation.y < -0.002 && pose.translation.y > -0.003);
}

#[test]
fn camera_projection_preserves_immediate_local_gaze_and_smooths_small_corrections() {
    let local_gaze = bevy::prelude::Quat::from_rotation_y(0.7);
    let mut camera = Transform::from_rotation(local_gaze);
    let mut projection = CameraTranslationProjection::default();

    project_bound_camera_translation(
        Vec3::new(3.0, 4.0, 5.0),
        1.0 / 60.0,
        &mut projection,
        &mut camera,
    );
    assert_eq!(camera.translation, Vec3::new(3.0, 4.0, 5.0));
    project_bound_camera_translation(
        Vec3::new(3.5, 4.0, 5.0),
        1.0 / 60.0,
        &mut projection,
        &mut camera,
    );

    assert!(camera.translation.x > 3.0 && camera.translation.x < 3.5);
    assert!(camera.rotation.abs_diff_eq(local_gaze, f32::EPSILON));
}

#[test]
fn render_update_does_not_advance_predicted_physics() {
    let mut app = App::new();
    app.init_resource::<Time<Fixed>>()
        .init_resource::<Time>()
        .init_resource::<ClientPlayerController>()
        .add_plugins(CreaturePredictionPlugin);
    set_authority(app.world_mut(), Some(1), Some(snapshot(1, [0.0; 3])));
    restore_authoritative_creature(app.world_mut());
    let creature = app.world().resource::<PredictionRuntime>().entity.unwrap();
    let before = app.world().get::<Transform>(creature).unwrap().translation;

    app.update();

    let after = app.world().get::<Transform>(creature).unwrap().translation;
    assert_eq!(
        after, before,
        "Update must not run the fixed Creature solver"
    );
}

#[test]
fn a_new_session_accepts_a_lower_tick_after_replacing_the_old_baseline() {
    let mut world = World::new();
    world.init_resource::<PredictionRuntime>();
    world.init_resource::<PredictionInputs>();
    world.init_resource::<PendingAuthoritySnapshot>();
    world.init_resource::<CameraTranslationProjection>();
    world.init_resource::<Time<Fixed>>();
    set_authority(&mut world, Some(1), Some(snapshot(100, [100.0, 0.0, 0.0])));
    restore_authoritative_creature(&mut world);

    set_authority(&mut world, Some(2), Some(snapshot(1, [1.0, 0.0, 0.0])));
    restore_authoritative_creature(&mut world);

    let runtime = world.resource::<PredictionRuntime>();
    assert_eq!(runtime.session_epoch, Some(2));
    assert_eq!(runtime.applied_tick, Some(1));
    let transform = world.get::<Transform>(runtime.entity.unwrap()).unwrap();
    assert_eq!(transform.translation, Vec3::X);
}

#[test]
fn disconnect_clears_entity_and_journal() {
    let mut world = World::new();
    world.init_resource::<PredictionRuntime>();
    world.init_resource::<PredictionInputs>();
    world.init_resource::<PendingAuthoritySnapshot>();
    world.init_resource::<Time<Fixed>>();
    world.init_resource::<CameraTranslationProjection>();
    set_authority(&mut world, Some(1), Some(snapshot(1, [0.0; 3])));
    restore_authoritative_creature(&mut world);
    world.resource_mut::<PredictionInputs>().append_tick(2);
    set_authority(&mut world, None, None);

    restore_authoritative_creature(&mut world);

    assert!(world.resource::<PredictionRuntime>().entity.is_none());
    assert_eq!(world.resource::<PredictionInputs>().journal_len(), 0);
}
