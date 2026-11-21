//! Movement readiness only. This adapter does not select block targets, consume
//! block-operation commands or submit voxel edits.
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
    let aabb = p.aabb;
    // Cover the whole AABB and the configured maximum movement sweep.
    // No interaction reach or camera ray contributes to physics demand.
    let radius =
        aabb.center_offset().length() + aabb.half_extents().length() + p.maximum_sweep_distance;
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
