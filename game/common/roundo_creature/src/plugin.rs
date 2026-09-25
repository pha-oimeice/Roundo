use crate::{
    BodyInstanceIds, BodyOrientation, ContactTolerance, EnvironmentSample, ExecutionBodyId,
    GazeAction, GazeControl, GazeIntent, GazeOrientation, GroundHorizontalMovement, GroundJump,
    GroundLocomotion, IntentBodyId, KinematicVelocity, LifeLink, LifeRegistry, LocomotionState,
    MovementAction, MovementIntent, RecordBodyId,
    collision::move_capsule,
    formula::{snap_distance, surface_direction, surface_is_legal},
};
use avian3d::{
    math::{AdjustPrecision as _, AsF32 as _},
    prelude::{
        Collider, MoveAndSlide, RigidBody, Sensor, ShapeCastConfig, SpatialQuery,
        SpatialQueryFilter,
    },
};
use bevy::{
    ecs::schedule::ScheduleLabel,
    prelude::{
        App, Component, Dir3, Entity, FixedUpdate, IntoScheduleConfigs, Plugin, Quat, Query, Res,
        SystemSet, Time, Transform, Vec3, With, World,
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum CreatureMotionSet {
    Solve,
    Commit,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ScheduleLabel)]
struct CreatureMotionSchedule;

/// Installs the sole fixed-tick writer for Creature pose, velocity and gaze.
pub struct CreatureMotionPlugin;
impl Plugin for CreatureMotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BodyInstanceIds>()
            .init_resource::<LifeRegistry>()
            .init_schedule(CreatureMotionSchedule)
            .add_systems(
                CreatureMotionSchedule,
                (resolve_life_intents, solve_creatures).chain(),
            )
            .configure_sets(
                FixedUpdate,
                (CreatureMotionSet::Solve, CreatureMotionSet::Commit).chain(),
            )
            .add_systems(
                FixedUpdate,
                commit_creature_motion.in_set(CreatureMotionSet::Commit),
            );
    }
}

/// Advances only Creature motion, without re-entering the application's global
/// fixed-update schedule. Authority calls this through [`CreatureMotionPlugin`];
/// prediction replay calls it directly for each retained local tick.
pub fn commit_creature_motion(world: &mut World) {
    world.run_schedule(CreatureMotionSchedule);
}

fn resolve_life_intents(
    mut intents: Query<(&MovementIntent, &mut GazeIntent, &LifeLink), With<IntentBodyId>>,
    mut executions: Query<(&mut MovementAction, &mut GazeAction, &LifeLink), With<ExecutionBodyId>>,
    records: Query<&LifeLink, With<RecordBodyId>>,
) {
    for (movement, mut gaze, link) in &mut intents {
        let entities = link.entities();
        let Ok((mut movement_action, mut gaze_action, execution_link)) =
            executions.get_mut(entities.execution)
        else {
            continue;
        };
        let Ok(record_link) = records.get(entities.record) else {
            continue;
        };
        if execution_link != link || record_link != link {
            continue;
        }
        *movement_action = (*movement).into();
        *gaze_action = (*gaze).into();
        *gaze = GazeIntent::default();
    }
}

#[derive(Component, Clone, Copy)]
struct GroundContact {
    normal: Vec3,
}

/// Prevents held vertical input from triggering another jump immediately after landing.
#[derive(Component, Clone, Copy)]
struct JumpInputLatch;

#[allow(clippy::type_complexity)]
fn solve_creatures(
    time: Res<Time<bevy::prelude::Fixed>>,
    mover: MoveAndSlide,
    spatial: SpatialQuery,
    sensors: Query<(), bevy::prelude::With<Sensor>>,
    mut creatures: Query<
        (
            Entity,
            &mut KinematicVelocity,
            &mut Transform,
            &mut BodyOrientation,
            &mut GazeOrientation,
            Option<&MovementAction>,
            Option<(&mut GazeAction, &GazeControl)>,
            Option<&EnvironmentSample>,
            &mut LocomotionState,
            Option<(&GroundLocomotion, &GroundHorizontalMovement)>,
            Option<&GroundJump>,
            &ContactTolerance,
            Option<&GroundContact>,
            Option<&JumpInputLatch>,
            &Collider,
        ),
        With<RigidBody>,
    >,
    mut commands: bevy::prelude::Commands,
) {
    let dt = time.delta();
    let dt_seconds = dt.as_secs_f32();
    if dt.is_zero() {
        return;
    }
    for (
        entity,
        mut velocity,
        mut transform,
        mut body,
        mut gaze,
        movement,
        mut gaze_intent,
        environment,
        mut state,
        ground_ability,
        jump_ability,
        tolerance,
        previous_contact,
        jump_latch,
        capsule,
    ) in &mut creatures
    {
        if !valid_motion_inputs(velocity.0, transform.translation) {
            continue;
        }
        let environment = environment.copied().unwrap_or_default();
        if let Some((intent, _)) = &mut gaze_intent {
            let yaw = Quat::from_axis_angle(body.0 * Vec3::Y, intent.yaw_delta);
            let yawed = (yaw * gaze.0).normalize();
            let pitch = Quat::from_axis_angle(yawed * Vec3::X, intent.pitch_delta);
            gaze.0 = (pitch * yawed).normalize();
            **intent = GazeAction::default();
        }

        let support_force = environment.gravity;
        let support_direction = support_force.normalize_or_zero();
        let fallback = previous_contact.map(|contact| contact.normal);
        let support = fallback.or_else(|| {
            ground_normal(
                &spatial,
                &sensors,
                entity,
                capsule,
                &transform,
                tolerance.0,
                support_direction,
            )
        });
        let grounded_support = support.filter(|normal| surface_is_legal(support_force, *normal));
        let input = movement.copied().unwrap_or_default();
        let jump_requested = input.local_axis[1] > 0.1;
        if !jump_requested && jump_latch.is_some() {
            commands.entity(entity).remove::<JumpInputLatch>();
        }

        // Gravity and collision remain physical. Ground control is a bounded
        // velocity response layered on top, giving deterministic acceleration and
        // braking without injecting unbounded momentum.
        velocity.0 += support_force * dt_seconds;
        let mut jumped = false;
        if let (Some(normal), Some((settings, _))) = (grounded_support, ground_ability) {
            apply_ground_control(
                &mut velocity.0,
                input,
                *settings,
                gaze.0,
                body.0,
                normal,
                dt_seconds,
            );
            if jump_requested && jump_latch.is_none() && jump_ability.is_some() {
                let tangent = velocity.0 - normal * velocity.0.dot(normal);
                velocity.0 = tangent + normal * settings.jump_speed;
                jumped = true;
                commands.entity(entity).insert(JumpInputLatch);
            }
        }

        // The capsule's long axis follows environmental down/up only. Horizontal
        // steering must never tilt the collider and change its resting footprint.
        let body_up = -support_direction;
        let sweep_body = (body_up.length_squared() > f32::EPSILON)
            .then(|| Quat::from_rotation_arc(Vec3::Y, body_up));
        if let Some(orientation) = sweep_body {
            transform.rotation = orientation;
        }

        let result = move_capsule(
            &mover,
            entity,
            capsule,
            transform.translation,
            transform.rotation,
            velocity.0,
            dt,
        );
        transform.translation = result.position;
        velocity.0 = result.velocity;

        // A deliberate jump cannot be immediately reclassified by its old floor.
        let contact = if jumped {
            None
        } else {
            result
                .normals
                .into_iter()
                .find(|normal| surface_is_legal(support_force, *normal))
                .or_else(|| {
                    grounded_support.and_then(|normal| {
                        let tangent = velocity.0 - normal * velocity.0.dot(normal);
                        let distance = snap_distance(tangent.length() * dt_seconds, tolerance.0);
                        ground_normal(
                            &spatial,
                            &sensors,
                            entity,
                            capsule,
                            &transform,
                            distance,
                            support_direction,
                        )
                        .filter(|candidate| surface_is_legal(support_force, *candidate))
                    })
                })
        };
        if let Some(normal) = contact {
            *state = LocomotionState::Grounded;
            commands.entity(entity).insert(GroundContact { normal });
        } else {
            *state = LocomotionState::Floating;
            commands.entity(entity).remove::<GroundContact>();
        }
        if let Some(orientation) = sweep_body {
            body.0 = orientation;
        }
    }
}

fn valid_motion_inputs(velocity: Vec3, position: Vec3) -> bool {
    velocity.is_finite() && position.is_finite()
}

fn apply_ground_control(
    velocity: &mut Vec3,
    input: MovementAction,
    settings: GroundLocomotion,
    gaze: Quat,
    body: Quat,
    normal: Vec3,
    dt_seconds: f32,
) {
    let axis = Vec3::from_array(input.local_axis);
    let strength = Vec3::new(axis.x, 0.0, axis.z).length().min(1.0);
    let heading = body * Vec3::NEG_Z;
    let direction = surface_direction(axis, gaze, normal, heading);
    let target_tangent = direction * settings.max_speed * strength;
    let current_normal = normal * velocity.dot(normal);
    let current_tangent = *velocity - current_normal;
    let acceleration = if strength > f32::EPSILON {
        settings.acceleration
    } else {
        settings.braking_acceleration
    };
    let delta = target_tangent - current_tangent;
    let next_tangent = current_tangent + delta.clamp_length_max(acceleration * dt_seconds);
    *velocity = current_normal + next_tangent;
}

fn ground_normal(
    spatial: &SpatialQuery,
    sensors: &Query<(), bevy::prelude::With<Sensor>>,
    entity: Entity,
    capsule: &Collider,
    transform: &Transform,
    distance: f32,
    direction: Vec3,
) -> Option<Vec3> {
    let direction = Dir3::new(direction).ok()?;
    let config = ShapeCastConfig::from_max_distance(distance.max(0.0));
    spatial
        .cast_shape_predicate(
            capsule,
            transform.translation.adjust_precision(),
            transform.rotation.adjust_precision(),
            direction,
            &config,
            &SpatialQueryFilter::from_excluded_entities([entity]),
            &|hit| !sensors.contains(hit),
        )
        .map(|hit| hit.normal1.f32())
}
