//! Nonexclusive bootstrap data/registration and publication phases. assembly.rs
//! owns their order and complete compensation; lifecycle.rs supplies exact
//! Factory/Slot/Controller request/result steps without a second progress ledger.
use super::*;
use bevy::prelude::{In, ParamSet, ResMut};
use roundo_creature::{IntentBodyId, RecordBodyId};
use roundo_life::{
    LifeEventManagement, LifeEventRegistrations, LifeInstances, LifeStateManagement,
    LifeStateReader, LifeStateRegistrations,
};

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct BootstrapRegistrations<'w, 's> {
    movement: LifeStateRegistrations<'w, 's, MovementCommandState>,
    gaze: LifeEventRegistrations<'w, 's, GazeCommandEvent>,
    look: LifeStateRegistrations<'w, 's, GazeCommandState>,
    experience: LifeStateRegistrations<'w, 's, CreatureMotionExperienceState>,
}
impl BootstrapRegistrations<'_, '_> {
    fn validate(&self, assembly: UnpublishedCreatureLife) -> Result<(), CreatureAssemblyError> {
        let life = assembly.life;
        let m = assembly.members;
        self.movement.validate_writer(life, m.intent)?;
        self.movement.validate_reader(life, m.execution)?;
        self.look.validate_writer(life, m.intent)?;
        self.look.validate_reader(life, m.intent)?;
        self.look.validate_reader(life, m.execution)?;
        self.gaze.validate_writer(life, m.intent)?;
        self.gaze.validate_reader(life, m.execution)?;
        self.experience.validate_writer(life, m.execution)?;
        self.experience.validate_reader(life, m.record)?;
        Ok(())
    }
}

/// Unpublished, composition-owned construction facts; never an authorization grant.
#[derive(Clone, Copy)]
pub(super) struct UnpublishedCreatureLife {
    pub(super) life: LifeHandle,
    pub(super) members: CreatureAssemblyMembers,
}
impl UnpublishedCreatureLife {
    pub(super) fn resolve(
        &self,
        instances: &LifeInstances,
    ) -> Result<[Entity; 3], CreatureAssemblyError> {
        instances.resolve_life(self.life)?;
        let m = self.members;
        if m.intent == m.execution || m.intent == m.record || m.execution == m.record {
            return Err(CreatureAssemblyError::InvalidBootstrap);
        }
        Ok([
            instances.resolve_body(m.intent)?,
            instances.resolve_body(m.execution)?,
            instances.resolve_body(m.record)?,
        ])
    }
    fn route(self) -> FactoryMotionRoute {
        FactoryMotionRoute::new(
            self.life,
            self.members.intent,
            self.members.execution,
            self.members.record,
        )
    }
}

/// Reads the real spawn baseline and approves only Movement topology. Owned
/// output may cross the structural Slot barrier, but is not an assembly receipt.
/// Missing/stale identities, roles or invalid values reject before mutation.
pub(super) fn prepare_creature_data(
    In(assembly): In<UnpublishedCreatureLife>,
    instances: LifeInstances,
    tick: Res<AuthoritativeFixedTick>,
    intents: Query<(), With<IntentBodyId>>,
    executions: Query<
        (
            &Transform,
            &BodyOrientation,
            &GazeOrientation,
            &KinematicVelocity,
            &LocomotionState,
        ),
        With<ExecutionBodyId>,
    >,
    records: Query<(), With<RecordBodyId>>,
    mut managers: ParamSet<(
        LifeStateManagement<MovementCommandState>,
        LifeStateManagement<GazeCommandState>,
    )>,
    mut commands: Commands,
) -> Result<CreatureMotionExperienceState, CreatureAssemblyError> {
    let [intent, execution, record] = assembly.resolve(&instances)?;
    intents
        .get(intent)
        .map_err(|_| CreatureAssemblyError::InvalidBootstrap)?;
    records
        .get(record)
        .map_err(|_| CreatureAssemblyError::InvalidBootstrap)?;
    let (transform, body, gaze, velocity, locomotion) = executions
        .get(execution)
        .map_err(|_| CreatureAssemblyError::InvalidBootstrap)?;
    let baseline = CreatureMotionExperienceState::new(
        *tick,
        transform.translation.to_array(),
        body.0.to_array(),
        gaze.0.to_array(),
        velocity.0.to_array(),
        *locomotion,
    )?;
    let mut look = managers.p1();
    look.approve_origin(assembly.life, assembly.members.intent)?;
    look.approve_reader(assembly.life, assembly.members.intent)?;
    look.approve_reader(assembly.life, assembly.members.execution)?;
    drop(look);
    let mut movement = managers.p0();
    movement.approve_origin(assembly.life, assembly.members.intent)?;
    movement.approve_reader(assembly.life, assembly.members.execution)?;
    commands
        .entity(record)
        .insert(CreatureMotionRecord::from_experience(baseline));
    Ok(baseline)
}

/// Runs after the Movement Slot structural barrier, preserving the old sequence
/// of registration, Gaze initialization and Experience origin/reader approval.
pub(super) fn prepare_creature_experience(
    In(assembly): In<UnpublishedCreatureLife>,
    instances: LifeInstances,
    mut managers: ParamSet<(
        LifeStateManagement<MovementCommandState>,
        LifeEventManagement<GazeCommandEvent>,
        LifeStateManagement<CreatureMotionExperienceState>,
        LifeStateManagement<GazeCommandState>,
    )>,
) -> Result<(), CreatureAssemblyError> {
    assembly.resolve(&instances)?;
    let mut look = managers.p3();
    look.register_reader(assembly.life, assembly.members.intent)?;
    look.register_reader(assembly.life, assembly.members.execution)?;
    drop(look);
    managers
        .p0()
        .register_reader(assembly.life, assembly.members.execution)?;
    let mut gaze = managers.p1();
    gaze.approve_writer(assembly.life, assembly.members.intent)?;
    gaze.approve_reader(assembly.life, assembly.members.execution)?;
    gaze.register_writer(assembly.life, assembly.members.intent)?;
    gaze.register_reader(assembly.life, assembly.members.execution)?;
    drop(gaze);
    let mut experience = managers.p2();
    experience.approve_origin(assembly.life, assembly.members.execution)?;
    experience.approve_reader(assembly.life, assembly.members.record)?;
    Ok(())
}

/// Runs after the Experience Slot structural barrier. Only a complete typed
/// topology gets a route marker; the assembly/Access is still unpublished.
pub(super) fn finish_creature_data(
    In(assembly): In<UnpublishedCreatureLife>,
    instances: LifeInstances,
    mut params: ParamSet<(
        LifeStateManagement<CreatureMotionExperienceState>,
        BootstrapRegistrations,
    )>,
    mut commands: Commands,
) -> Result<(), CreatureAssemblyError> {
    let [intent, _, _] = assembly.resolve(&instances)?;
    params
        .p0()
        .register_reader(assembly.life, assembly.members.record)?;
    params.p1().validate(assembly)?;
    commands.entity(intent).insert(assembly.route());
    Ok(())
}

fn validate_controllers(
    ids: TestLifeControllers,
    intent: ConceptBodyHandle,
    registry: &ControllerRegistry,
    controllers: &Query<&Controller>,
) -> Result<(), CreatureAssemblyError> {
    for (id, domain) in [
        (ids.movement, IntentDomain::movement()),
        (ids.gaze, IntentDomain::gaze()),
    ] {
        let entity = registry
            .controller_entity(id)
            .ok_or(CreatureAssemblyError::InvalidBootstrap)?;
        let controller = controllers
            .get(entity)
            .map_err(|_| CreatureAssemblyError::InvalidBootstrap)?;
        if controller.id() != id
            || controller.intent_body_handle() != Some(intent)
            || controller.domain() != domain
            || controller.current_owner().is_some()
        {
            return Err(CreatureAssemblyError::InvalidBootstrap);
        }
    }
    Ok(())
}

pub(super) fn attach_test_life_controllers(
    In((assembly, ids)): In<(UnpublishedCreatureLife, TestLifeControllers)>,
    instances: LifeInstances,
    registry: Res<ControllerRegistry>,
    controllers: Query<&Controller>,
    mut commands: Commands,
) -> Result<(), CreatureAssemblyError> {
    let [_, execution, _] = assembly.resolve(&instances)?;
    validate_controllers(ids, assembly.members.intent, &registry, &controllers)?;
    commands.entity(execution).insert(ids);
    Ok(())
}

/// Revalidates current identity, initialized data/route, exact Controller targets
/// and the registry-backed Player before publishing any mapping or Access. The
/// synchronous bridge flushes targeted markers before exposing a success result.
pub(super) fn publish_test_life(
    In((assembly, player_entity, ids)): In<(UnpublishedCreatureLife, Entity, TestLifeControllers)>,
    instances: LifeInstances,
    controller_registry: Res<ControllerRegistry>,
    controllers: Query<&Controller>,
    markers: Query<&TestLifeControllers, With<ExecutionBodyId>>,
    routes: Query<(&FactoryMotionRoute, &IntentBodyId)>,
    records: Query<(&CreatureMotionRecord, &RecordBodyId)>,
    player_registry: Res<PlayerRegistry>,
    mut players: Query<&mut Player>,
    mut assemblies: ResMut<CreatureAssemblyRegistry>,
    movement: LifeStateReader<MovementCommandState>,
    experience: LifeStateReader<CreatureMotionExperienceState>,
    registrations: BootstrapRegistrations,
    mut commands: Commands,
) -> Result<LifeHandle, CreatureAssemblyError> {
    let [intent, execution, record] = assembly.resolve(&instances)?;
    registrations.validate(assembly)?;
    validate_controllers(
        ids,
        assembly.members.intent,
        &controller_registry,
        &controllers,
    )?;
    if routes.get(intent).map(|(route, _)| *route).ok() != Some(assembly.route())
        || markers.get(execution).copied().ok() != Some(ids)
    {
        return Err(CreatureAssemblyError::InvalidBootstrap);
    }
    let (record_value, _) = records
        .get(record)
        .map_err(|_| CreatureAssemblyError::InvalidBootstrap)?;
    let baseline = instances
        .with_body(record, |body| {
            experience.with_state_view(&body, assembly.life, |view| view.current().copied())
        })??
        .ok_or(CreatureAssemblyError::InvalidBootstrap)?;
    if *record_value != CreatureMotionRecord::from_experience(baseline) {
        return Err(CreatureAssemblyError::InvalidBootstrap);
    }
    let initialized = instances.with_body(execution, |body| {
        movement.with_state_view(&body, assembly.life, |view| view.current().is_some())
    })??;
    if !initialized {
        return Err(CreatureAssemblyError::InvalidBootstrap);
    }
    let mut player = players
        .get_mut(player_entity)
        .map_err(|_| CreatureAssemblyError::InvalidBootstrap)?;
    if player_registry.player_entity(player.id()) != Some(player_entity) {
        return Err(CreatureAssemblyError::InvalidBootstrap);
    }
    assemblies.insert_with_controllers(assembly.life, assembly.members, ids)?;
    player.set_controller_access(ids.movement, ControllerAccessLevel::ReadWrite);
    player.set_controller_access(ids.gaze, ControllerAccessLevel::ReadWrite);
    commands.entity(player_entity).insert(BootstrappedTestLife {
        life: assembly.life,
    });
    Ok(assembly.life)
}

/// Compensation clears only composition-owned mappings and targeted markers.
/// Controller removal and Factory invalidation remain in their owning modules.
pub(super) fn clear_failed_bootstrap(
    In((life, execution)): In<(LifeHandle, ConceptBodyHandle)>,
    instances: LifeInstances,
    mut cleanup: AssemblyCleanup,
) {
    cleanup.remove_mapping(life);
    if let Ok(entity) = instances.resolve_body(execution) {
        cleanup
            .commands
            .entity(entity)
            .try_remove::<TestLifeControllers>();
    }
}
