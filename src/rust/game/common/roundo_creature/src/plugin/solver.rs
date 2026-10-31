//! World-aligned AABB motion solving and local physical input validation.

use super::*;

/// The same derived shape is used by Avian physics, sweeps and support probes.
/// Runs even for blocked/duplicate motion ticks: gating cannot leave a rotated
/// collider or an externally replaced shape installed on a Creature.
pub(super) fn sync_movement_aabbs(
    mut creatures: Query<(Ref<AabbDimensions>, &mut Collider, &mut Transform), With<RigidBody>>,
) {
    for (dimensions, mut collider, mut transform) in &mut creatures {
        if dimensions.is_changed() || collider.is_changed() {
            *collider = dimensions.collider();
        }
        if transform.rotation != Quat::IDENTITY {
            transform.rotation = Quat::IDENTITY;
        }
        if transform.scale != Vec3::ONE {
            transform.scale = Vec3::ONE;
        }
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn solve_creatures(
    time: Res<Time<bevy::prelude::Fixed>>,
    authoritative_tick: Option<Res<AuthoritativeFixedTick>>,
    mut solve_context: ResMut<FactoryMotionSolveContext>,
    mut commit_proofs: ResMut<FactoryMotionCommitProofs>,
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
            (
                &Collider,
                Option<(&ConceptBodyIdentity, &FactoryMotionSolvedTick)>,
                Option<&JumpInputMode>,
                Option<&FactoryMotionPolicy>,
                Option<&FactoryMotionReadiness>,
                Option<&DiscreteJumpAttempt>,
            ),
        ),
        (With<RigidBody>, With<AabbDimensions>),
    >,
    mut commands: bevy::prelude::Commands,
) {
    let dt = time.delta();
    let dt_seconds = dt.as_secs_f32();
    let shared_tick = authoritative_tick.as_deref().copied();
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
        (aabb, solved_tick, jump_mode, policy, readiness, discrete_attempt),
    ) in &mut creatures
    {
        let solved_marker = solved_tick.and_then(|(identity, marker)| {
            (identity.handle() == marker.execution).then_some(*marker)
        });
        if solved_marker.is_some_and(|marker| shared_tick.is_none_or(|tick| marker.tick == tick)) {
            continue;
        }
        let execution_context = solve_context.0.get(&entity).copied();
        let discrete = jump_mode == Some(&JumpInputMode::Discrete);
        let gated = policy == Some(&FactoryMotionPolicy::RequireReadiness);
        if gated
            && !execution_context.is_some_and(|context| {
                shared_tick == Some(context.tick)
                    && readiness.is_some_and(|gate| gate.permits(context.route, context.tick))
            })
        {
            if discrete {
                commands.entity(entity).remove::<DiscreteJumpAttempt>();
                if let Some((action, _)) = &mut gaze_intent {
                    **action = GazeAction::default();
                }
            }
            continue;
        }
        if discrete
            && (!gated
                || !execution_context.is_some_and(|context| {
                    discrete_attempt.is_some_and(|attempt| {
                        attempt.route == context.route
                            && attempt.tick == context.tick
                            && attempt.valid
                    }) && movement.is_some_and(|input| input.local_axis[1] == 0.0)
                }))
        {
            continue;
        }
        if execution_context.is_some_and(|context| {
            solve_context.0.get(&entity).is_some_and(|current| {
                current.route == context.route && current.tick == context.tick && current.committed
            }) || commit_proofs
                .0
                .get(&context.route.execution)
                .is_some_and(|proof| proof.route == context.route && proof.tick == context.tick)
        }) {
            continue;
        }
        if !valid_motion_inputs(velocity.0, transform.translation) {
            continue;
        }
        let capture_context = execution_context.filter(|_| {
            dt_seconds.is_finite()
                && dt_seconds > 0.0
                && body.0.is_normalized()
                && gaze.0.is_normalized()
                && movement.copied().is_some_and(valid_movement_action)
                && gaze_intent
                    .as_ref()
                    .is_some_and(|(action, _)| valid_gaze_action(**action))
                && environment.copied().is_some_and(valid_environment_sample)
        });
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
        // Discrete requests must use actual current support. In particular a
        // pre-edit GroundContact cannot grant a jump after its floor disappears.
        let fallback = (!discrete)
            .then(|| previous_contact.map(|contact| contact.normal))
            .flatten();
        let support = fallback.or_else(|| {
            ground_normal(
                &spatial,
                &sensors,
                entity,
                aabb,
                &transform,
                tolerance.0,
                support_direction,
            )
        });
        let grounded_support = support.filter(|normal| surface_is_legal(support_force, *normal));
        let input = movement.copied().unwrap_or_default();
        let jump_requested = if discrete {
            discrete_attempt.is_some_and(|attempt| attempt.requested)
        } else {
            input.local_axis[1] > 0.1
        };
        if !discrete && !jump_requested && jump_latch.is_some() {
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
            if jump_requested && (discrete || jump_latch.is_none()) && jump_ability.is_some() {
                let tangent = velocity.0 - normal * velocity.0.dot(normal);
                velocity.0 = tangent + normal * settings.jump_speed;
                jumped = true;
                if !discrete {
                    commands.entity(entity).insert(JumpInputLatch);
                }
            }
        }

        // Environmental up defines the locomotion frame only. The movement
        // AABB always retains the world's axes, including under sideways gravity.
        let body_up = -support_direction;
        let locomotion_body = (body_up.length_squared() > f32::EPSILON)
            .then(|| Quat::from_rotation_arc(Vec3::Y, body_up));

        let result = move_aabb(&mover, entity, aabb, transform.translation, velocity.0, dt);
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
                            aabb,
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
        if let Some(orientation) = locomotion_body {
            body.0 = orientation;
        }
        if let Some(context) = execution_context
            && let Some(current) = solve_context.0.get_mut(&entity)
            && current.route == context.route
            && current.tick == context.tick
        {
            current.committed = true;
        }
        let completed_marker = execution_context
            .map(|context| FactoryMotionSolvedTick {
                execution: context.route.execution,
                tick: context.tick,
            })
            .or_else(|| {
                solved_marker
                    .zip(shared_tick)
                    .map(|(marker, tick)| FactoryMotionSolvedTick {
                        execution: marker.execution,
                        tick,
                    })
            });
        if let Some(marker) = completed_marker {
            commands.entity(entity).insert(marker);
        }
        if let Some(context) = capture_context
            && let Ok(experience) = crate::CreatureMotionExperienceState::new(
                context.tick,
                transform.translation.to_array(),
                body.0.to_array(),
                gaze.0.to_array(),
                velocity.0.to_array(),
                *state,
            )
        {
            commit_proofs.0.insert(
                context.route.execution,
                FactoryMotionCommitProof {
                    route: context.route,
                    tick: context.tick,
                    experience,
                    environment,
                    fixed_dt_seconds: dt_seconds,
                    published: false,
                    recorded: false,
                },
            );
        }
    }
}

fn valid_motion_inputs(velocity: Vec3, position: Vec3) -> bool {
    velocity.is_finite() && position.is_finite()
}

fn valid_movement_action(action: MovementAction) -> bool {
    let axis = Vec3::from_array(action.local_axis);
    axis.is_finite() && axis.length_squared() <= 1.0 + f32::EPSILON
}

fn valid_gaze_action(action: GazeAction) -> bool {
    action.yaw_delta.is_finite() && action.pitch_delta.is_finite()
}

fn valid_environment_sample(environment: EnvironmentSample) -> bool {
    environment.gravity.is_finite()
        && environment.static_friction.is_finite()
        && environment.kinetic_friction.is_finite()
        && environment.static_friction >= 0.0
        && environment.kinetic_friction >= 0.0
        && environment.kinetic_friction <= environment.static_friction
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
    aabb: &Collider,
    transform: &Transform,
    distance: f32,
    direction: Vec3,
) -> Option<Vec3> {
    let direction = Dir3::new(direction).ok()?;
    let config = ShapeCastConfig::from_max_distance(distance.max(0.0));
    spatial
        .cast_shape_predicate(
            aabb,
            transform.translation.adjust_precision(),
            Quat::IDENTITY.adjust_precision(),
            direction,
            &config,
            &SpatialQueryFilter::from_excluded_entities([entity]),
            &|hit| !sensors.contains(hit),
        )
        .map(|hit| hit.normal1.f32())
}
