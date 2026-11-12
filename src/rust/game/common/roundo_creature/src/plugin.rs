mod solver;
use solver::*;

#[derive(Component, Clone, Copy)]
struct GroundContact {
    normal: Vec3,
}

/// Prevents held vertical input from triggering another jump immediately after landing.
#[derive(Component, Clone, Copy)]
struct JumpInputLatch;

/// The current projection only; air/NotReady attempts cannot survive a tick.
#[derive(Component, Clone, Copy)]
struct DiscreteJumpAttempt {
    route: FactoryMotionRoute,
    tick: AuthoritativeFixedTick,
    requested: bool,
    valid: bool,
}

use crate::{
    AabbDimensions, BodyInstanceIds, BodyOrientation, ContactTolerance, EnvironmentSample,
    ExecutionBodyId, FactoryMotionPolicy, FactoryMotionReadiness, GazeAction, GazeCommandEvent,
    GazeCommandState, GazeControl, GazeOrientation, GroundHorizontalMovement, GroundJump,
    GroundLocomotion, IntentBodyId, JumpCommandEvent, JumpInputMode, KinematicVelocity,
    LocomotionState, MovementAction, MovementCommandState, RecordBodyId,
    collision::move_aabb,
    formula::{snap_distance, surface_direction, surface_is_legal},
};
use avian3d::{
    math::ToRealPrecision as _,
    prelude::{
        Collider, MoveAndSlide, RigidBody, Sensor, ShapeCastConfig, SpatialQuery,
        SpatialQueryFilter,
    },
};
use bevy::{
    ecs::{schedule::ScheduleLabel, system::SystemParam},
    prelude::{
        App, Commands, Component, DetectChanges, Dir3, Entity, FixedUpdate, In,
        IntoScheduleConfigs, Plugin, Quat, Query, Ref, Res, ResMut, Resource, SystemSet, Time,
        Transform, Vec3, With, World,
    },
};
use roundo_life::{
    AuthoritativeFixedTick, ConceptBodyFactory, ConceptBodyHandle, ConceptBodyIdentity,
    LifeEventReader, LifeFactory, LifeFixedSet, LifeHandle, LifeInstances, LifeStateReader,
    LifeStateWriter,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum CreatureMotionSet {
    Solve,
    Commit,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum CreatureLifeSet {
    CommandProjection,
    MotionCommit,
    ExperiencePublish,
    RecordUpdate,
    Projection,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ScheduleLabel)]
struct CreatureMotionSchedule;

/// Private ownership marker for neutralizing Actions once this exact Factory
/// Execution generation has been admitted to route projection.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
struct FactoryProjectedActionOwner(ConceptBodyHandle, FactoryMotionRoute);

/// Physical completion fact for this exact Factory Execution generation and
/// authoritative tick. It is deliberately independent of route permissions.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
struct FactoryMotionSolvedTick {
    execution: ConceptBodyHandle,
    tick: AuthoritativeFixedTick,
}

#[derive(Clone, Copy)]
struct ResolvedFactoryMotionRoute {
    execution: Entity,
    record: Entity,
}

#[derive(Clone, Copy)]
struct FactoryMotionExecutionContext {
    route: FactoryMotionRoute,
    tick: AuthoritativeFixedTick,
    committed: bool,
}

#[derive(Resource, Default)]
struct FactoryMotionSolveContext(HashMap<Entity, FactoryMotionExecutionContext>);

#[derive(Clone, Copy)]
struct FactoryMotionCommitProof {
    route: FactoryMotionRoute,
    tick: AuthoritativeFixedTick,
    experience: crate::CreatureMotionExperienceState,
    environment: EnvironmentSample,
    fixed_dt_seconds: f32,
    published: bool,
    recorded: bool,
}

#[derive(Resource, Default)]
struct FactoryMotionCommitProofs(HashMap<ConceptBodyHandle, FactoryMotionCommitProof>);

/// Trusted composition cleanup for exact activation-derived motion facts. A
/// route is a correlation target, not authority; Body behavior never receives
/// this control-plane parameter. It grants no membership or data registration.
#[derive(SystemParam)]
pub struct FactoryMotionLifecycle<'w, 's> {
    instances: LifeInstances<'w, 's>,
    context: ResMut<'w, FactoryMotionSolveContext>,
    proofs: ResMut<'w, FactoryMotionCommitProofs>,
    owners: Query<
        'w,
        's,
        (
            Option<&'static FactoryProjectedActionOwner>,
            Option<&'static DiscreteJumpAttempt>,
            Option<&'static mut MovementAction>,
            Option<&'static mut GazeAction>,
        ),
    >,
    commands: Commands<'w, 's>,
}
impl FactoryMotionLifecycle<'_, '_> {
    /// Idempotently clears only matching route caches and exact-generation
    /// projected inputs. Deferred removal must apply before allowing reassembly.
    /// Physical solved-tick/contact/latch facts, permanent policy and committed
    /// pose/velocity/orientations are deliberately retained.
    pub fn clear_route(&mut self, route: FactoryMotionRoute) {
        self.context.0.retain(|_, context| context.route != route);
        if self
            .proofs
            .0
            .get(&route.execution)
            .is_some_and(|proof| proof.route == route)
        {
            self.proofs.0.remove(&route.execution);
        }
        let Ok(entity) = self.instances.resolve_body(route.execution) else {
            return;
        };
        let Ok((owner, attempt, movement, gaze)) = self.owners.get_mut(entity) else {
            return;
        };
        if owner.is_some_and(|owner| owner.0 == route.execution && owner.1 == route) {
            if let Some(mut action) = movement {
                *action = MovementAction::default();
            }
            if let Some(mut action) = gaze {
                *action = GazeAction::default();
            }
            self.commands
                .entity(entity)
                .remove::<FactoryProjectedActionOwner>();
        }
        if attempt.is_some_and(|attempt| attempt.route == route) {
            self.commands.entity(entity).remove::<DiscreteJumpAttempt>();
        }
    }
}

/// Owned read-only receipt for one proof that was published and accepted by its
/// Record Body during the currently validated authoritative tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommittedFactoryMotion {
    route: FactoryMotionRoute,
    experience: crate::CreatureMotionExperienceState,
    environment: EnvironmentSample,
    fixed_dt_seconds: f32,
}

impl CommittedFactoryMotion {
    pub const fn route(&self) -> FactoryMotionRoute {
        self.route
    }

    pub const fn experience(&self) -> crate::CreatureMotionExperienceState {
        self.experience
    }

    pub const fn environment(&self) -> EnvironmentSample {
        self.environment
    }

    pub const fn fixed_dt_seconds(&self) -> f32 {
        self.fixed_dt_seconds
    }
}

/// Installs the sole fixed-tick writer for Creature pose, velocity and gaze.
/// Factory-composed motion routing. Handles are the only persistent targets;
/// execution resolves them for the current operation.
#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct FactoryMotionRoute {
    life: LifeHandle,
    intent: ConceptBodyHandle,
    execution: ConceptBodyHandle,
    record: ConceptBodyHandle,
}

impl FactoryMotionRoute {
    pub const fn new(
        life: LifeHandle,
        intent: ConceptBodyHandle,
        execution: ConceptBodyHandle,
        record: ConceptBodyHandle,
    ) -> Self {
        Self {
            life,
            intent,
            execution,
            record,
        }
    }

    pub const fn life(self) -> LifeHandle {
        self.life
    }
    pub const fn intent(self) -> ConceptBodyHandle {
        self.intent
    }
    pub const fn execution(self) -> ConceptBodyHandle {
        self.execution
    }
    pub const fn record(self) -> ConceptBodyHandle {
        self.record
    }
}

/// Composition-owned, read-only contract for current assembly membership.
/// It is deliberately absent by default; it is neither a cached proof nor a
/// transferable permission, and direct replay must still validate membership.
pub trait FactoryMotionRouteValidator: Resource {
    /// Checks current composition membership only. Factory identity, actual route
    /// and role checks are performed separately by Creature's typed resolver.
    /// This is not Player authorization and must not cache a past admission.
    fn contains_route(&self, route: FactoryMotionRoute) -> bool;
}

type ReadOnlyRouteCheck = fn(&World, FactoryMotionRoute) -> bool;

/// A composition validator has already been installed in this App.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryMotionRouteValidatorInstallError {
    AlreadyInstalled,
}

#[derive(Resource, Default)]
struct FactoryMotionRouteValidation(Option<ReadOnlyRouteCheck>, Option<std::any::TypeId>);

/// Installs one typed composition adapter at App construction time.
/// Runtime systems borrow only `V`, not World. Missing membership resources
/// reject every route; installation never mints lifecycle/data authority.
/// A second installation is rejected without replacing the original adapter.
pub fn install_factory_motion_route_validator<V: FactoryMotionRouteValidator>(
    app: &mut App,
) -> Result<(), FactoryMotionRouteValidatorInstallError> {
    if app
        .world()
        .get_resource::<FactoryMotionRouteValidation>()
        .is_some_and(|value| value.1.is_some())
    {
        return Err(FactoryMotionRouteValidatorInstallError::AlreadyInstalled);
    }
    fn check<V: FactoryMotionRouteValidator>(world: &World, route: FactoryMotionRoute) -> bool {
        world
            .get_resource::<V>()
            .is_some_and(|membership| membership.contains_route(route))
    }
    app.insert_resource(FactoryMotionRouteValidation(
        Some(check::<V>),
        Some(std::any::TypeId::of::<V>()),
    ))
    .init_resource::<FactoryMotionSolveContext>()
    .init_resource::<FactoryMotionCommitProofs>()
    .init_schedule(CreatureMotionSchedule)
    .add_systems(
        FixedUpdate,
        (
            project_factory_commands::<V>.in_set(CreatureLifeSet::CommandProjection),
            project_factory_gaze::<V>
                .after(project_factory_commands::<V>)
                .in_set(CreatureLifeSet::CommandProjection),
            prepare_factory_motion_context::<V>
                .in_set(CreatureLifeSet::MotionCommit)
                .after(discard_stale_factory_motion_solved_ticks)
                .before(solve_creatures),
            publish_factory_experience::<V>.in_set(CreatureLifeSet::ExperiencePublish),
            update_factory_records::<V>.in_set(CreatureLifeSet::RecordUpdate),
        ),
    )
    .add_systems(
        CreatureMotionSchedule,
        prepare_factory_motion_context::<V>
            .after(discard_stale_factory_motion_solved_ticks)
            .before(solve_creatures),
    );
    Ok(())
}

/// Read-only typed route access for ECS systems. Creature knows role/data
/// requirements; the composition-owned resource knows membership. All accesses
/// are visible to Bevy and evaluated at each operation. Missing installation,
/// resource, Factory instance, role or exact route fails closed.
#[derive(SystemParam)]
pub struct FactoryMotionRoutes<'w, 's, V: FactoryMotionRouteValidator> {
    instances: LifeInstances<'w, 's>,
    validation: Option<Res<'w, FactoryMotionRouteValidation>>,
    membership: Option<Res<'w, V>>,
    roles: Query<
        'w,
        's,
        (
            Option<&'static IntentBodyId>,
            Option<&'static ExecutionBodyId>,
            Option<&'static RecordBodyId>,
        ),
    >,
    routes: Query<'w, 's, (Entity, &'static FactoryMotionRoute)>,
    modes: Query<'w, 's, &'static JumpInputMode>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
}

impl<V: FactoryMotionRouteValidator> FactoryMotionRoutes<'_, '_, V> {
    /// Instantaneous Factory/membership/role validation, not Player authorization
    /// or a transferable capability. Recheck at each operation, not just startup.
    pub fn is_live(&self, route: FactoryMotionRoute) -> bool {
        self.instances
            .resolve_body(route.intent)
            .ok()
            .is_some_and(|intent| self.resolve(intent, route).is_some())
    }

    /// Returns an owned held command through this route's currently registered
    /// Execution reader. This does not alter Actions, history, tick or receipt,
    /// and grants neither Player Read access nor lifecycle administration.
    pub fn read_movement_command(
        &self,
        movement: &LifeStateReader<MovementCommandState>,
        route: FactoryMotionRoute,
    ) -> Option<MovementCommandState> {
        let intent = self.instances.resolve_body(route.intent).ok()?;
        let resolved = self.resolve(intent, route)?;
        self.instances
            .with_body(resolved.execution, |body| {
                movement.with_state_view(&body, route.life, |view| view.current().copied())
            })
            .ok()?
            .ok()?
    }

    fn resolve(
        &self,
        intent_entity: Entity,
        route: FactoryMotionRoute,
    ) -> Option<ResolvedFactoryMotionRoute> {
        let validation = self.validation.as_ref()?;
        if validation.0.is_none() || validation.1 != Some(std::any::TypeId::of::<V>()) {
            return None;
        }
        let membership = self.membership.as_ref()?;
        if self.instances.resolve_body(route.intent).ok()? != intent_entity
            || self.routes.get(intent_entity).ok()?.1 != &route
            || !membership.contains_route(route)
        {
            return None;
        }
        self.instances.resolve_life(route.life).ok()?;
        let execution = self.instances.resolve_body(route.execution).ok()?;
        let record = self.instances.resolve_body(route.record).ok()?;
        let intent_roles = self.roles.get(intent_entity).ok()?;
        let execution_roles = self.roles.get(execution).ok()?;
        let record_roles = self.roles.get(record).ok()?;
        if !(intent_roles.0.is_some()
            && intent_roles.1.is_none()
            && intent_roles.2.is_none()
            && execution_roles.0.is_none()
            && execution_roles.1.is_some()
            && execution_roles.2.is_none()
            && record_roles.0.is_none()
            && record_roles.1.is_none()
            && record_roles.2.is_some())
        {
            return None;
        }
        Some(ResolvedFactoryMotionRoute { execution, record })
    }

    fn live_routes(&self) -> Vec<(Entity, FactoryMotionRoute)> {
        self.routes
            .iter()
            .filter_map(|(entity, route)| self.resolve(entity, *route).map(|_| (entity, *route)))
            .collect()
    }
}

pub struct CreatureMotionPlugin;
impl Plugin for CreatureMotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BodyInstanceIds>()
            .init_resource::<FactoryMotionRouteValidation>()
            .init_resource::<FactoryMotionSolveContext>()
            .init_resource::<FactoryMotionCommitProofs>()
            .init_schedule(CreatureMotionSchedule)
            .add_systems(
                CreatureMotionSchedule,
                (
                    discard_stale_factory_motion_solved_ticks.before(solve_creatures),
                    sync_movement_aabbs.before(solve_creatures),
                    solve_creatures,
                ),
            )
            .configure_sets(
                FixedUpdate,
                (CreatureMotionSet::Solve, CreatureMotionSet::Commit).chain(),
            )
            .configure_sets(
                FixedUpdate,
                (
                    CreatureLifeSet::CommandProjection,
                    CreatureLifeSet::MotionCommit,
                    CreatureLifeSet::ExperiencePublish,
                    CreatureLifeSet::RecordUpdate,
                    CreatureLifeSet::Projection,
                )
                    .chain()
                    .after(LifeFixedSet::CommitFactories),
            )
            .add_systems(
                FixedUpdate,
                (
                    discard_stale_factory_motion_solved_ticks
                        .in_set(CreatureLifeSet::MotionCommit)
                        .before(solve_creatures),
                    sync_movement_aabbs
                        .in_set(CreatureLifeSet::MotionCommit)
                        .before(solve_creatures),
                    solve_creatures
                        .in_set(CreatureLifeSet::MotionCommit)
                        .in_set(CreatureMotionSet::Commit),
                ),
            );
    }
}

/// Advances only Creature motion, without re-entering the application's global
/// fixed-update schedule. Prediction replay still calls this directly for each
/// retained local tick; only a validated Factory route gets shared-tick context.
pub fn commit_creature_motion(world: &mut World) {
    world.run_schedule(CreatureMotionSchedule);
    world.flush();
}

fn discard_stale_factory_motion_solved_ticks(
    instances: LifeInstances,
    markers: Query<(Entity, &FactoryMotionSolvedTick)>,
    mut commands: Commands,
) {
    for (entity, marker) in &markers {
        if instances.resolve_body(marker.execution).ok() != Some(entity) {
            commands.entity(entity).remove::<FactoryMotionSolvedTick>();
        }
    }
}

fn resolve_live_factory_motion_route(
    world: &World,
    intent_entity: Entity,
    route: FactoryMotionRoute,
) -> Option<ResolvedFactoryMotionRoute> {
    let validator = world.get_resource::<FactoryMotionRouteValidation>()?.0?;
    let resolved_intent = ConceptBodyFactory::resolve_entity(world, route.intent).ok()?;
    let execution = ConceptBodyFactory::resolve_entity(world, route.execution).ok()?;
    let record = ConceptBodyFactory::resolve_entity(world, route.record).ok()?;
    LifeFactory::resolve_entity(world, route.life).ok()?;

    let intent_identity = world.get::<ConceptBodyIdentity>(intent_entity)?;
    let execution_identity = world.get::<ConceptBodyIdentity>(execution)?;
    let record_identity = world.get::<ConceptBodyIdentity>(record)?;
    let roles_match = world.get::<IntentBodyId>(intent_entity).is_some()
        && world.get::<ExecutionBodyId>(intent_entity).is_none()
        && world.get::<RecordBodyId>(intent_entity).is_none()
        && world.get::<IntentBodyId>(execution).is_none()
        && world.get::<ExecutionBodyId>(execution).is_some()
        && world.get::<RecordBodyId>(execution).is_none()
        && world.get::<IntentBodyId>(record).is_none()
        && world.get::<ExecutionBodyId>(record).is_none()
        && world.get::<RecordBodyId>(record).is_some();
    if resolved_intent != intent_entity
        || intent_identity.handle() != route.intent
        || execution_identity.handle() != route.execution
        || record_identity.handle() != route.record
        || ConceptBodyFactory::handle_for_entity(world, intent_entity) != Some(route.intent)
        || ConceptBodyFactory::handle_for_entity(world, execution) != Some(route.execution)
        || ConceptBodyFactory::handle_for_entity(world, record) != Some(route.record)
        || world.get::<FactoryMotionRoute>(intent_entity) != Some(&route)
        || !roles_match
        || !LifeFactory::is_live(world, route.life)
        || !validator(world, route)
    {
        return None;
    }
    Some(ResolvedFactoryMotionRoute { execution, record })
}

type ProjectedActions<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static FactoryProjectedActionOwner>,
        Option<&'static mut MovementAction>,
        Option<&'static mut GazeAction>,
    ),
    bevy::prelude::Or<(With<ExecutionBodyId>, With<FactoryProjectedActionOwner>)>,
>;

/// Mirrors an Intent-owned look selection without integrating camera velocity.
/// Legacy recipes without this State retain their existing relative input path.
fn project_factory_gaze<V: FactoryMotionRouteValidator>(
    routes: FactoryMotionRoutes<V>,
    gaze: LifeStateReader<GazeCommandState>,
    mut executions: Query<(&mut GazeOrientation, &mut GazeAction)>,
) {
    for (intent, route) in routes.live_routes() {
        let Some(resolved) = routes.resolve(intent, route) else {
            continue;
        };
        let selected = routes
            .instances
            .with_body(resolved.execution, |body| {
                gaze.with_state_view(&body, route.life, |view| view.current().copied())
            })
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|value| value.orientation());
        if let Some(selected) = selected {
            if let Ok((mut orientation, mut action)) = executions.get_mut(resolved.execution) {
                orientation.0 = Quat::from_array(selected);
                *action = GazeAction::default();
            }
        }
    }
}

fn project_factory_commands<V: FactoryMotionRouteValidator>(
    routes: FactoryMotionRoutes<V>,
    movement: LifeStateReader<MovementCommandState>,
    gaze: LifeEventReader<GazeCommandEvent>,
    jump: LifeEventReader<JumpCommandEvent>,
    actions: ProjectedActions,
    commands: Commands,
) {
    let candidates = routes.live_routes();
    project_factory_command_routes(
        In(candidates),
        routes,
        movement,
        gaze,
        jump,
        actions,
        commands,
    );
}

fn project_factory_command_routes<V: FactoryMotionRouteValidator>(
    In(candidates): In<Vec<(Entity, FactoryMotionRoute)>>,
    routes: FactoryMotionRoutes<V>,
    movement: LifeStateReader<MovementCommandState>,
    mut gaze: LifeEventReader<GazeCommandEvent>,
    mut jump: LifeEventReader<JumpCommandEvent>,
    mut actions: ProjectedActions,
    mut commands: Commands,
) {
    let mut admitted = Vec::new();
    let mut new_owners = HashMap::new();
    for (intent_entity, route) in candidates {
        let Some(resolved) = routes.resolve(intent_entity, route) else {
            continue;
        };
        let Ok((_, _, movement, gaze)) = actions.get(resolved.execution) else {
            continue;
        };
        if movement.is_none() || gaze.is_none() {
            continue;
        }
        new_owners.insert(resolved.execution, route.execution);
        commands
            .entity(resolved.execution)
            .insert(FactoryProjectedActionOwner(route.execution, route));
        admitted.push((intent_entity, route));
    }

    // Include newly admitted owners immediately, without waiting for Commands.
    // Neutralize all exact-generation owners before projection; an invalid later
    // route cannot erase a valid earlier projection or touch an unowned Body.
    for (entity, previous, movement, gaze) in &mut actions {
        let Some(owner) = new_owners
            .get(&entity)
            .copied()
            .or_else(|| previous.map(|owner| owner.0))
        else {
            continue;
        };
        let roles = routes.roles.get(entity).ok();
        let current = routes.instances.resolve_body(owner).ok() == Some(entity)
            && roles
                .is_some_and(|roles| roles.0.is_none() && roles.1.is_some() && roles.2.is_none());
        if !current {
            commands
                .entity(entity)
                .remove::<FactoryProjectedActionOwner>();
            continue;
        }
        if let Some(mut movement) = movement {
            *movement = MovementAction::default();
        }
        if let Some(mut gaze) = gaze {
            *gaze = GazeAction::default();
        }
    }

    for (intent_entity, route) in admitted {
        let Some(resolved) = routes.resolve(intent_entity, route) else {
            continue;
        };
        let discrete = routes.modes.get(resolved.execution).ok() == Some(&JumpInputMode::Discrete);
        let values = routes
            .instances
            .with_body(resolved.execution, |body| {
                let movement = movement
                    .with_state_view(&body, route.life, |view| view.current().copied())
                    .ok()??;
                let first = gaze.read_next_with(&body, route.life, Clone::clone).ok()?;
                let mut gaze_action = first.map_or_else(GazeAction::default, Into::into);
                let mut requested = false;
                let mut valid = true;
                if discrete {
                    // Each FIFO is hard bounded by its validated native policy. The
                    // new recipe consumes the whole one-tick intake, not latest-wins.
                    while let Some(value) =
                        gaze.read_next_with(&body, route.life, Clone::clone).ok()?
                    {
                        gaze_action.yaw_delta += value.yaw_delta();
                        gaze_action.pitch_delta += value.pitch_delta();
                    }
                    while jump
                        .read_next_with(&body, route.life, |_| ())
                        .ok()?
                        .is_some()
                    {
                        requested = true;
                    }
                    valid = movement.local_axis()[1] == 0.0
                        && gaze_action.yaw_delta.is_finite()
                        && gaze_action.pitch_delta.is_finite();
                    if !valid {
                        gaze_action = GazeAction::default();
                    }
                }
                Some((movement, gaze_action, requested, valid))
            })
            .ok()
            .flatten();
        let Some((movement, gaze_action_value, requested, valid)) = values else {
            if discrete && let Some(tick) = routes.tick.as_deref().copied() {
                commands
                    .entity(resolved.execution)
                    .insert(DiscreteJumpAttempt {
                        route,
                        tick,
                        requested: false,
                        valid: false,
                    });
            }
            continue;
        };
        if discrete && let Some(tick) = routes.tick.as_deref().copied() {
            commands
                .entity(resolved.execution)
                .insert(DiscreteJumpAttempt {
                    route,
                    tick,
                    requested,
                    valid,
                });
        }
        let Some(current) = routes.resolve(intent_entity, route) else {
            continue;
        };
        if current.execution != resolved.execution {
            continue;
        }
        let Ok((_, _, Some(mut movement_action), Some(mut gaze_action))) =
            actions.get_mut(resolved.execution)
        else {
            continue;
        };
        *movement_action = movement.into();
        *gaze_action = gaze_action_value;
    }
}

fn prepare_factory_motion_context<V: FactoryMotionRouteValidator>(
    tick: Option<Res<AuthoritativeFixedTick>>,
    routes: FactoryMotionRoutes<V>,
    mut context: ResMut<FactoryMotionSolveContext>,
    mut proofs: ResMut<FactoryMotionCommitProofs>,
) {
    let tick = tick.as_deref().copied();
    let mut next = HashMap::new();
    if let Some(tick) = tick {
        for (intent_entity, route) in routes.live_routes() {
            let Some(resolved) = routes.resolve(intent_entity, route) else {
                continue;
            };
            let committed = context.0.get(&resolved.execution).is_some_and(|previous| {
                previous.route == route && previous.tick == tick && previous.committed
            });
            next.insert(
                resolved.execution,
                FactoryMotionExecutionContext {
                    route,
                    tick,
                    committed,
                },
            );
        }
        proofs.0.retain(|handle, proof| {
            proof.tick == tick
                && next.values().any(|context| {
                    context.route.execution == *handle && context.route == proof.route
                })
        });
    } else {
        proofs.0.clear();
    }
    context.0 = next;
}

/// Returns a receipt only after this tick's typed Experience publish and strict
/// Record replacement have both succeeded for the same still-live route.
pub fn committed_factory_motion(
    world: &World,
    route: FactoryMotionRoute,
) -> Option<CommittedFactoryMotion> {
    let tick = *world.get_resource::<AuthoritativeFixedTick>()?;
    let intent_entity = ConceptBodyFactory::resolve_entity(world, route.intent).ok()?;
    let resolved = resolve_live_factory_motion_route(world, intent_entity, route)?;
    let proof = *world
        .get_resource::<FactoryMotionCommitProofs>()?
        .0
        .get(&route.execution)?;
    let record = world.get::<crate::CreatureMotionRecord>(resolved.record)?;
    proof.receipt(route, tick, record)
}

/// Read-only access to current committed motion. A receipt requires a live route,
/// this tick's successful publish, strict Record acceptance and an exact Record
/// match. It conveys no Player authorization or lifecycle authority.
#[derive(SystemParam)]
pub struct FactoryMotionReceipts<'w, 's, V: FactoryMotionRouteValidator> {
    routes: FactoryMotionRoutes<'w, 's, V>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
    proofs: Option<Res<'w, FactoryMotionCommitProofs>>,
    records: Query<'w, 's, &'static crate::CreatureMotionRecord>,
}

impl<V: FactoryMotionRouteValidator> FactoryMotionReceipts<'_, '_, V> {
    pub fn committed_motion(&self, route: FactoryMotionRoute) -> Option<CommittedFactoryMotion> {
        let tick = **self.tick.as_ref()?;
        let intent = self.routes.instances.resolve_body(route.intent).ok()?;
        let resolved = self.routes.resolve(intent, route)?;
        let proof = self.proofs.as_ref()?.0.get(&route.execution)?;
        let record = self.records.get(resolved.record).ok()?;
        proof.receipt(route, tick, record)
    }
}

/// Physical commit evidence captured by Execution, independent of typed
/// publication and Recorder policy. Possession conveys no edit permission.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExecutedFactoryMotion {
    experience: crate::CreatureMotionExperienceState,
    environment: EnvironmentSample,
}
impl ExecutedFactoryMotion {
    pub const fn experience(self) -> crate::CreatureMotionExperienceState {
        self.experience
    }
    pub const fn environment(self) -> EnvironmentSample {
        self.environment
    }
}

/// Read-only host observation of this tick's physical commit, independent of
/// Recorder output. It does not mint data/edit access or reconstruct a proof.
pub fn executed_factory_motion(
    world: &World,
    route: FactoryMotionRoute,
) -> Option<ExecutedFactoryMotion> {
    let tick = *world.get_resource::<AuthoritativeFixedTick>()?;
    let intent = ConceptBodyFactory::resolve_entity(world, route.intent).ok()?;
    resolve_live_factory_motion_route(world, intent, route)?;
    let proof = world
        .get_resource::<FactoryMotionCommitProofs>()?
        .0
        .get(&route.execution)?;
    (proof.route == route && proof.tick == tick).then_some(ExecutedFactoryMotion {
        experience: proof.experience,
        environment: proof.environment,
    })
}

/// Reads this tick's immutable physical commit, not a reconstructed Transform
/// or a Recorder receipt. Every call revalidates the exact live Factory route.
#[derive(SystemParam)]
pub struct FactoryMotionExecutions<'w, 's, V: FactoryMotionRouteValidator> {
    routes: FactoryMotionRoutes<'w, 's, V>,
    tick: Option<Res<'w, AuthoritativeFixedTick>>,
    proofs: Option<Res<'w, FactoryMotionCommitProofs>>,
}
impl<V: FactoryMotionRouteValidator> FactoryMotionExecutions<'_, '_, V> {
    pub fn executed_motion(&self, route: FactoryMotionRoute) -> Option<ExecutedFactoryMotion> {
        if !self.routes.is_live(route) {
            return None;
        }
        let tick = **self.tick.as_ref()?;
        let proof = self.proofs.as_ref()?.0.get(&route.execution)?;
        (proof.route == route && proof.tick == tick).then_some(ExecutedFactoryMotion {
            experience: proof.experience,
            environment: proof.environment,
        })
    }
}

impl FactoryMotionCommitProof {
    fn receipt(
        &self,
        route: FactoryMotionRoute,
        tick: AuthoritativeFixedTick,
        record: &crate::CreatureMotionRecord,
    ) -> Option<CommittedFactoryMotion> {
        if self.route != route || self.tick != tick || !self.published || !self.recorded {
            return None;
        }
        (*record == crate::CreatureMotionRecord::from_experience(self.experience)).then_some(
            CommittedFactoryMotion {
                route,
                experience: self.experience,
                environment: self.environment,
                fixed_dt_seconds: self.fixed_dt_seconds,
            },
        )
    }
}

/// Reports whether this route passes the current composition validator and every
/// live Factory/body identity check. This is an instantaneous routing fact only;
/// it is not an authorization capability or a committed-motion receipt, and must
/// not be cached across operations or ticks.
pub fn is_live_factory_motion_route(world: &World, route: FactoryMotionRoute) -> bool {
    let Ok(intent_entity) = ConceptBodyFactory::resolve_entity(world, route.intent()) else {
        return false;
    };
    resolve_live_factory_motion_route(world, intent_entity, route).is_some()
}

fn publish_factory_experience<V: FactoryMotionRouteValidator>(
    tick: Option<Res<AuthoritativeFixedTick>>,
    routes: FactoryMotionRoutes<V>,
    mut experience: LifeStateWriter<crate::CreatureMotionExperienceState>,
    mut proofs: ResMut<FactoryMotionCommitProofs>,
) {
    let Some(tick) = tick.as_deref().copied() else {
        proofs.0.clear();
        return;
    };
    proofs.0.retain(|handle, proof| {
        proof.tick == tick
            && proof.route.execution == *handle
            && routes
                .instances
                .resolve_body(proof.route.intent)
                .ok()
                .is_some_and(|intent| routes.resolve(intent, proof.route).is_some())
    });
    for proof in proofs.0.values_mut() {
        if proof.published {
            continue;
        }
        let Ok(execution) = routes.instances.resolve_body(proof.route.execution) else {
            continue;
        };
        let result = routes.instances.with_body(execution, |body| {
            experience.set_state(&body, proof.route.life, proof.experience)
        });
        if matches!(result, Ok(Ok(()))) {
            proof.published = true;
        }
    }
}

fn update_factory_records<V: FactoryMotionRouteValidator>(
    tick: Option<Res<AuthoritativeFixedTick>>,
    routes: FactoryMotionRoutes<V>,
    experience: LifeStateReader<crate::CreatureMotionExperienceState>,
    mut records: Query<&mut crate::CreatureMotionRecord>,
    mut proofs: ResMut<FactoryMotionCommitProofs>,
) {
    let current_tick = tick.as_deref().copied();
    proofs.0.retain(|handle, proof| {
        proof.route.execution == *handle
            && routes
                .instances
                .resolve_body(proof.route.intent)
                .ok()
                .is_some_and(|intent| routes.resolve(intent, proof.route).is_some())
    });
    for (intent_entity, route) in routes.live_routes() {
        let Some(resolved) = routes.resolve(intent_entity, route) else {
            continue;
        };
        // A registered reader is not a recording policy. An empty Recorder
        // does not read Life data merely because this route can resolve it.
        if !records.contains(resolved.record) {
            continue;
        }
        let value = routes
            .instances
            .with_body(resolved.record, |body| {
                experience.with_state_view(&body, route.life, |view| view.current().copied())
            })
            .ok()
            .and_then(Result::ok)
            .flatten();
        let Some(value) = value else {
            continue;
        };
        let Some(current) = routes.resolve(intent_entity, route) else {
            continue;
        };
        if current.record != resolved.record {
            continue;
        }
        let Ok(mut record) = records.get_mut(resolved.record) else {
            continue;
        };
        if value.committed_tick() <= record.committed_tick() {
            continue;
        }
        *record = crate::CreatureMotionRecord::from_experience(value);
        if current_tick == Some(value.committed_tick())
            && let Some(proof) = proofs.0.get_mut(&route.execution)
            && proof.route == route
            && Some(proof.tick) == current_tick
            && proof.published
            && proof.experience == value
        {
            proof.recorded = true;
        }
    }
}
