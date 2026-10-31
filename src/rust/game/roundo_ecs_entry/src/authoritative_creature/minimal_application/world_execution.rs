//! World readiness and behavior are Execution concerns, not Command admission
//! or Recorder policy. No request/response mapping is kept back to Intent.
use super::*;

pub(super) fn refresh_readiness(
    instances: LifeInstances,
    routes: FactoryMotionRoutes<CreatureAssemblyRegistry>,
    tick: Res<AuthoritativeFixedTick>,
    time: Res<Time<Fixed>>,
    environment: Option<Res<VirtualChunkEnvironmentMap>>,
    coordinates: Query<&GlobalTransform>,
    mut demands: LocalPhysicsDemands,
    mut apps: ResMut<Applications>,
    mut executions: Query<(
        &ApplicationOwner,
        &Transform,
        &KinematicVelocity,
        &GroundLocomotion,
        &mut EnvironmentSample,
        &mut FactoryMotionReadiness,
    )>,
) {
    for (&id, entry) in &mut apps.entries {
        let Ok(entity) = instances.resolve_body(entry.members.execution) else {
            continue;
        };
        let Ok((owner, pose, velocity, ability, mut sample, mut gate)) = executions.get_mut(entity)
        else {
            continue;
        };
        gate.block();
        if owner.0 != id
            || entry.status != MinimalLifeStatus::Active
            || !routes.is_live(entry.members.motion_route())
        {
            continue;
        }
        let Some((values, revision)) = environment
            .as_ref()
            .and_then(|map| map.sample(pose.translation))
        else {
            continue;
        };
        let Ok(current) = EnvironmentSample::new(
            values.gravity,
            values.static_friction,
            values.kinetic_friction,
            revision,
        ) else {
            continue;
        };
        let dt = time.delta_secs();
        let sweep = ability.sweep_distance_bound(velocity.0, current.gravity, dt);
        if !dt.is_finite()
            || dt <= 0.0
            || !sweep.is_finite()
            || sweep > entry.parameters.0.maximum_sweep_distance
        {
            continue;
        }
        let Ok(transform) = coordinates.get(entry.coordinate.entity()) else {
            continue;
        };
        let Some(region) = required_region(entry, pose.translation, transform, &demands) else {
            continue;
        };
        let Some(demand) = entry.demand else {
            continue;
        };
        if demands.update(demand, region).is_err() {
            continue;
        }
        entry.region = region;
        if demands.readiness(entry.coordinate, region) == Ok(PhysicsRegionReadiness::Ready) {
            *sample = current;
            gate.set_ready(entry.members.motion_route(), *tick);
        }
    }
}

fn required_region(
    entry: &Application,
    position: Vec3,
    transform: &GlobalTransform,
    demands: &LocalPhysicsDemands,
) -> Option<PhysicsDemandRegion> {
    let affine = transform.affine();
    if !affine.matrix3.determinant().is_finite()
        || affine.matrix3.determinant().abs() <= f32::EPSILON
    {
        return None;
    }
    let inverse = affine.inverse();
    let p = entry.parameters.0;
    let aabb = p.viewpoint.aabb();
    // Covers any gaze produced this tick, the whole AABB and the configured
    // maximum sweep, not just the destination. Transform all eight region corners.
    let radius = aabb.center_offset().length()
        + aabb.half_extents().length()
        + p.maximum_sweep_distance
        + p.viewpoint.eye_offset().length()
        + p.viewpoint.maximum_distance();
    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                let point = inverse.transform_point3(position + Vec3::new(x, y, z) * radius);
                minimum = minimum.min(point);
                maximum = maximum.max(point);
            }
        }
    }
    if !minimum.is_finite() || !maximum.is_finite() {
        return None;
    }
    let neighbors = i32::try_from(p.neighbor_chunks).ok()?;
    let chunk = |point: Vec3, sign: i32| -> Option<IVec3> {
        let mut result = [0; 3];
        for (index, axis) in point.to_array().into_iter().enumerate() {
            let value = (f64::from(axis) / CHUNK_EDGE_LENGTH as f64).floor();
            // Voxel positions and physics mesh origins must remain representable.
            if value < f64::from(i32::MIN / CHUNK_EDGE_LENGTH as i32)
                || value > f64::from(i32::MAX / CHUNK_EDGE_LENGTH as i32)
            {
                return None;
            }
            result[index] = (value as i32).checked_add(sign.checked_mul(neighbors)?)?;
        }
        Some(IVec3::from_array(result))
    };
    demands
        .budgets()
        .ok()?
        .physics_demands()
        .region(chunk(minimum, -1)?, chunk(maximum, 1)?)
        .ok()
}

pub(super) fn execute_interactions(
    instances: LifeInstances,
    routes: FactoryMotionRoutes<CreatureAssemblyRegistry>,
    motions: FactoryMotionExecutions<CreatureAssemblyRegistry>,
    apps: Res<Applications>,
    owners: Query<&ApplicationOwner>,
    coordinates: Query<&GlobalTransform>,
    registry: Option<Res<AtomicVoxelRegistry>>,
    mut commands: LifeEventReader<BlockInteractionCommandEvent>,
    mut world: ParamSet<(VoxelRaycaster, LocalVoxelEdits)>,
) {
    // Order applications deterministically; per-Life Destroy/Place retain their
    // native FIFO order. No other host's LC head is drained or bypassed.
    let mut ids = apps.entries.keys().copied().collect::<Vec<_>>();
    ids.sort_by_key(|id| {
        instances
            .resolve_body(apps.entries[id].members.execution)
            .ok()
    });
    for id in ids {
        let entry = &apps.entries[&id];
        if entry.status != MinimalLifeStatus::Active
            || !entry.parameters.0.edit_permission
            || !routes.is_live(entry.members.motion_route())
        {
            continue;
        }
        let Ok(entity) = instances.resolve_body(entry.members.execution) else {
            continue;
        };
        if owners.get(entity).map(|owner| owner.0).ok() != Some(id) {
            continue;
        }
        let Some(motion) = motions.executed_motion(entry.members.motion_route()) else {
            continue;
        };
        let Ok(ray) = entry.parameters.0.viewpoint.ray(motion.experience()) else {
            continue;
        };
        let limit = entry.parameters.0.interaction.limits();
        let mut edits = 0;
        for _ in 0..limit.consume_per_tick {
            if edits >= limit.edits_per_tick || !world.p1().can_submit_and_commit() {
                break;
            }
            let value = instances.with_body(entity, |body| {
                commands.read_next_with(&body, entry.members.life, |value| value.operation())
            });
            let Ok(Ok(Some(operation))) = value else {
                break;
            };
            let Some(hit) = world
                .p0()
                .cast(ray, entry.parameters.0.viewpoint.maximum_distance())
            else {
                continue;
            };
            if hit.chunk.local_coordinate_entity() != entry.coordinate.entity() {
                continue;
            }
            let position = if operation.is_destroy() {
                hit.voxel_position()
            } else {
                hit.previous_voxel_position
            };
            if !operation.is_destroy() {
                if !registry
                    .as_ref()
                    .is_some_and(|voxels| voxels.permits_placement(operation.replacement()))
                {
                    continue;
                }
                let Ok(transform) = coordinates.get(entry.coordinate.entity()) else {
                    continue;
                };
                if intersects_aabb(
                    position,
                    transform,
                    motion.experience(),
                    entry.parameters.0.viewpoint.aabb(),
                ) {
                    continue;
                }
            }
            let mut local = world.p1();
            let Ok(target) = local.observe(entry.coordinate, position) else {
                continue;
            };
            if !operation.is_destroy() && target.expected().voxel != EMPTY_VOXEL_ID {
                continue;
            }
            let Ok(edit) = local.submit(target, operation.replacement()) else {
                continue;
            };
            // Irreversible boundary: revalidate now, not through a retained
            // admission token or Command lifetime. No callbacks run in between.
            let eligible = entry.status == MinimalLifeStatus::Active
                && entry.parameters.0.edit_permission
                && routes.is_live(entry.members.motion_route())
                && owners.get(entity).map(|owner| owner.0).ok() == Some(id);
            let completion = if eligible {
                local.commit_front(edit)
            } else {
                local.reject_front(edit, VoxelEditRejection::NotEligible)
            };
            completion.expect("same-system LC work reservation must terminalize its exact head");
            // Claim the actual world result, including failures, releasing the
            // LC slot. An empty Recorder neither reads nor keeps this fact.
            local
                .take_completion(edit)
                .expect("exact LC completion is retained until claim");
            edits += 1;
        }
    }
}

pub(in crate::authoritative_creature) fn intersects_aabb(
    position: IVec3,
    coordinate: &GlobalTransform,
    motion: CreatureMotionExperienceState,
    aabb: AabbDimensions,
) -> bool {
    // Conservative policy for transformed voxel boxes: false negatives are
    // forbidden; rejecting a near-AABB placement is preferable to overlap.
    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    for x in [0.0, 1.0] {
        for y in [0.0, 1.0] {
            for z in [0.0, 1.0] {
                let point = coordinate
                    .affine()
                    .transform_point3(position.as_vec3() + Vec3::new(x, y, z));
                minimum = minimum.min(point);
                maximum = maximum.max(point);
            }
        }
    }
    let center = aabb.center_at(Vec3::from_array(*motion.translation()));
    let extent = aabb.half_extents();
    minimum.cmple(center + extent).all() && maximum.cmpge(center - extent).all()
}
