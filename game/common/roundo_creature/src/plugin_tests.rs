use crate::{
    BodyOrientation, CreatureMotionPlugin, EnvironmentSample, ExecutionBodyId,
    GazeAction as GazeIntent, GazeControl, GazeOrientation, GroundHorizontalMovement,
    IntentBodyBundle, IntentBodyId, KinematicVelocity, LifeEntities, LocomotionState,
    MovementAction as MovementIntent, MovementIntent as BodyMovementIntent, RecordBodyBundle,
    RecordBodyId, TestExecutionBodyPrototype, compose_life,
};
use avian3d::prelude::{Collider, PhysicsPlugins, RigidBody, Sensor};
use bevy::{
    asset::{AssetEvent, AssetPlugin, Assets},
    prelude::{App, FixedUpdate, Mesh, MinimalPlugins, Time, Transform, Vec3},
};
use std::time::Duration;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        PhysicsPlugins::default(),
        CreatureMotionPlugin,
    ));
    app.add_message::<AssetEvent<Mesh>>();
    app.init_resource::<Assets<Mesh>>();
    // Avian's diagnostic registration is process-global while Rust tests build
    // several Apps in parallel; initialize the per-App resource explicitly.
    app.init_resource::<avian3d::collider_tree::ColliderTreeDiagnostics>();
    app.init_resource::<avian3d::collision::CollisionDiagnostics>();
    app.init_resource::<avian3d::dynamics::solver::SolverDiagnostics>();
    app.init_resource::<avian3d::spatial_query::SpatialQueryDiagnostics>();
    app.update(); // publish Transform-backed Avian collider positions before manual ticks.
    app
}

fn creature(app: &mut App, position: Vec3) -> bevy::prelude::Entity {
    app.world_mut()
        .spawn(
            TestExecutionBodyPrototype::default()
                .materialize_runtime(position, EnvironmentSample::default()),
        )
        .id()
}

fn tick(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time<bevy::time::Fixed>>()
        .advance_by(Duration::from_secs_f32(1.0 / 64.0));
    app.world_mut().run_schedule(FixedUpdate);
}

fn static_box(app: &mut App, position: Vec3, size: Vec3) {
    app.world_mut().spawn((
        Transform::from_translation(position),
        RigidBody::Static,
        Collider::cuboid(size.x, size.y, size.z),
    ));
}

#[test]
fn headless_fall_lands_and_grounded_rest_is_stable() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(2.0, 1.0, 20.0),
    );
    let creature = creature(&mut app, Vec3::new(0.0, 3.0, 0.0));
    for _ in 0..120 {
        tick(&mut app);
    }
    let world = app.world();
    assert_eq!(
        *world.get::<LocomotionState>(creature).unwrap(),
        LocomotionState::Grounded
    );
    let y = world.get::<Transform>(creature).unwrap().translation.y;
    assert!(
        y > 0.899 && y < 0.905,
        "grounded capsule separation is too large or unstable: {y}"
    );

    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::new([1.0, 0.0, 0.0]).unwrap());
    for _ in 0..150 {
        tick(&mut app);
    }
    assert_eq!(
        *app.world().get::<LocomotionState>(creature).unwrap(),
        LocomotionState::Floating,
        "leaving a finite support must not keep a stale grounded contact"
    );
}

#[test]
fn headless_sweep_stops_at_thin_wall_and_slides() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(0.05, 4.0, 4.0),
    );
    let creature = creature(&mut app, Vec3::ZERO);
    app.world_mut()
        .get_mut::<KinematicVelocity>(creature)
        .unwrap()
        .0 = Vec3::X * 500.0;
    tick(&mut app);
    let pose = app.world().get::<Transform>(creature).unwrap().translation;
    assert!(pose.x < 1.6, "continuous sweep crossed thin wall: {pose:?}");
    assert!(
        app.world()
            .get::<KinematicVelocity>(creature)
            .unwrap()
            .0
            .x
            .abs()
            < 1e-3
    );
}

#[test]
fn headless_sensor_does_not_block_and_initial_overlap_depenetrates() {
    let mut app = app();
    app.world_mut().spawn((
        Transform::from_translation(Vec3::X),
        RigidBody::Static,
        Collider::cuboid(0.1, 4.0, 4.0),
        Sensor,
    ));
    let mover = creature(&mut app, Vec3::ZERO);
    app.world_mut()
        .get_mut::<KinematicVelocity>(mover)
        .unwrap()
        .0 = Vec3::X * 100.0;
    tick(&mut app);
    assert!(app.world().get::<Transform>(mover).unwrap().translation.x > 1.2);

    let overlap_start = Vec3::new(5.7, 0.0, 0.0);
    let overlapping = creature(&mut app, overlap_start);
    static_box(&mut app, Vec3::new(5.0, 0.0, 0.0), Vec3::splat(1.0));
    app.update();
    tick(&mut app);
    assert_ne!(
        app.world()
            .get::<Transform>(overlapping)
            .unwrap()
            .translation,
        overlap_start
    );
}

#[test]
fn ability_markers_gate_movement_and_gaze_writes() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(20.0, 1.0, 20.0),
    );
    let creature = creature(&mut app, Vec3::new(0.0, 1.0, 0.0));
    app.world_mut()
        .entity_mut(creature)
        .remove::<GroundHorizontalMovement>()
        .remove::<GazeControl>()
        .insert((
            MovementIntent::new([1.0, 0.0, 0.0]).unwrap(),
            GazeIntent::new(0.5, 0.25).unwrap(),
        ));
    for _ in 0..8 {
        tick(&mut app);
    }
    let world = app.world();
    assert!(
        world
            .get::<Transform>(creature)
            .unwrap()
            .translation
            .x
            .abs()
            < 1e-4
    );
    assert_eq!(
        world.get::<GazeOrientation>(creature).unwrap().0,
        bevy::prelude::Quat::IDENTITY
    );
}

#[test]
fn sustained_ground_input_stays_grounded_without_vertical_jitter() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(100.0, 1.0, 100.0),
    );
    let creature = creature(&mut app, Vec3::new(0.0, 1.0, 0.0));
    for _ in 0..16 {
        tick(&mut app);
    }
    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::new([1.0, 0.0, 0.0]).unwrap());

    let mut minimum_y = f32::INFINITY;
    let mut maximum_y = f32::NEG_INFINITY;
    for _ in 0..64 {
        tick(&mut app);
        assert_eq!(
            *app.world().get::<LocomotionState>(creature).unwrap(),
            LocomotionState::Grounded
        );
        let y = app
            .world()
            .get::<Transform>(creature)
            .unwrap()
            .translation
            .y;
        minimum_y = minimum_y.min(y);
        maximum_y = maximum_y.max(y);
    }

    assert!(
        maximum_y - minimum_y < 0.001,
        "ground motion changed capsule height by {}",
        maximum_y - minimum_y
    );
    let body = app.world().get::<BodyOrientation>(creature).unwrap().0;
    assert!(body.abs_diff_eq(bevy::prelude::Quat::IDENTITY, 1e-5));
}

#[test]
fn releasing_ground_input_brakes_quickly_without_sliding() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(100.0, 1.0, 100.0),
    );
    let creature = creature(&mut app, Vec3::new(0.0, 1.0, 0.0));
    for _ in 0..16 {
        tick(&mut app);
    }
    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::new([1.0, 0.0, 0.0]).unwrap());
    for _ in 0..64 {
        tick(&mut app);
    }
    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::default());
    let release_position = app.world().get::<Transform>(creature).unwrap().translation;

    for _ in 0..16 {
        tick(&mut app);
    }

    let world = app.world();
    let stopped_position = world.get::<Transform>(creature).unwrap().translation;
    let speed = world.get::<KinematicVelocity>(creature).unwrap().0.length();
    assert!(speed < 0.1, "released Creature still moves at {speed}");
    assert!(
        stopped_position.distance(release_position) < 0.75,
        "released Creature slid from {release_position:?} to {stopped_position:?}"
    );
}

#[test]
fn upward_intent_jumps_once_until_released_and_can_jump_again_after_landing() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(100.0, 1.0, 100.0),
    );
    let creature = creature(&mut app, Vec3::new(0.0, 1.0, 0.0));
    for _ in 0..16 {
        tick(&mut app);
    }
    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::new([0.0, 1.0, 0.0]).unwrap());

    tick(&mut app);

    assert_eq!(
        *app.world().get::<LocomotionState>(creature).unwrap(),
        LocomotionState::Floating
    );
    assert!(app.world().get::<KinematicVelocity>(creature).unwrap().0.y > 5.0);
    for _ in 0..160 {
        tick(&mut app);
    }
    assert_eq!(
        *app.world().get::<LocomotionState>(creature).unwrap(),
        LocomotionState::Grounded,
        "held jump input must not repeatedly jump after landing"
    );

    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::default());
    tick(&mut app);
    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::new([0.0, 1.0, 0.0]).unwrap());
    tick(&mut app);

    assert_eq!(
        *app.world().get::<LocomotionState>(creature).unwrap(),
        LocomotionState::Floating
    );
    assert!(app.world().get::<KinematicVelocity>(creature).unwrap().0.y > 5.0);
}

#[test]
fn positive_local_forward_moves_along_gaze_negative_z() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(20.0, 1.0, 20.0),
    );
    let creature = creature(&mut app, Vec3::new(0.0, 1.0, 0.0));
    for _ in 0..16 {
        tick(&mut app);
    }
    let baseline = app.world().get::<Transform>(creature).unwrap().translation;
    app.world_mut()
        .entity_mut(creature)
        .insert(MovementIntent::new([0.0, 0.0, 1.0]).unwrap());
    for _ in 0..8 {
        tick(&mut app);
    }
    let moved = app.world().get::<Transform>(creature).unwrap().translation;
    assert!(
        moved.z < baseline.z,
        "positive local forward must follow gaze -Z: {baseline:?} -> {moved:?}"
    );
}

#[test]
fn headless_non_y_gravity_finds_support_and_commits_body_pose() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(1.5, 0.0, 0.0),
        Vec3::new(1.0, 20.0, 20.0),
    );
    let creature = creature(&mut app, Vec3::new(-1.0, 0.0, 0.0));
    app.world_mut()
        .entity_mut(creature)
        .insert(EnvironmentSample::new(Vec3::X * 9.81, 0.6, 0.5, 7).unwrap());
    for _ in 0..120 {
        tick(&mut app);
    }
    let world = app.world();
    assert_eq!(
        *world.get::<LocomotionState>(creature).unwrap(),
        LocomotionState::Grounded
    );
    let transform = world.get::<Transform>(creature).unwrap();
    assert!(
        transform.translation.x < 0.2,
        "crossed +X support: {transform:?}"
    );
    assert_eq!(
        transform.rotation,
        world.get::<BodyOrientation>(creature).unwrap().0
    );
}

#[test]
fn life_routes_intent_values_to_a_distinct_execution_body() {
    let mut app = app();
    static_box(
        &mut app,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(20.0, 1.0, 20.0),
    );
    let intent = app
        .world_mut()
        .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
        .id();
    let execution = app
        .world_mut()
        .spawn(TestExecutionBodyPrototype::default().materialize(
            ExecutionBodyId(1),
            Vec3::new(0.0, 1.0, 0.0),
            EnvironmentSample::default(),
        ))
        .id();
    let record = app
        .world_mut()
        .spawn(RecordBodyBundle::materialize(RecordBodyId(1)))
        .id();
    compose_life(
        app.world_mut(),
        LifeEntities {
            intent,
            execution,
            record,
        },
    )
    .unwrap();
    app.world_mut()
        .entity_mut(intent)
        .insert(BodyMovementIntent::new([1.0, 0.0, 0.0]).unwrap());

    for _ in 0..24 {
        tick(&mut app);
    }

    assert!(
        app.world()
            .get::<Transform>(execution)
            .unwrap()
            .translation
            .x
            > 0.1
    );
    assert!(app.world().get::<Transform>(intent).is_none());
    assert!(app.world().get::<BodyMovementIntent>(execution).is_none());
    assert!(app.world().get::<Transform>(record).is_none());
}
