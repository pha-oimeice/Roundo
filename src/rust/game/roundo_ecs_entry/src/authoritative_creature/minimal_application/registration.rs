//! Native preflight, approval/registration and exact-generation derived cleanup.
use super::*;
use bevy::ecs::system::SystemParam;

#[derive(SystemParam)]
pub(super) struct Evidence<'w, 's> {
    movement: LifeStateRegistrations<'w, 's, MovementCommandState>,
    motion: LifeStateRegistrations<'w, 's, CreatureMotionExperienceState>,
    gaze: LifeEventRegistrations<'w, 's, GazeCommandEvent>,
    jump: LifeEventRegistrations<'w, 's, JumpCommandEvent>,
    command: LifeEventRegistrations<'w, 's, BlockInteractionCommandEvent>,
    experience: LifeEventRegistrations<'w, 's, BlockInteractionExperienceEvent>,
}
fn pristine_event<E: EventData>(
    r: &LifeEventRegistrations<E>,
    life: LifeHandle,
    writer: ConceptBodyHandle,
    reader: Option<ConceptBodyHandle>,
    policy: EventPolicy,
) -> Result<u64, MinimalLifeCompositionError> {
    r.validate_writer_declaration(writer)?;
    if let Some(reader) = reader {
        r.validate_reader_declaration(reader)?;
    }
    let facts = r.interface_evidence(life)?;
    if facts.policy() != policy {
        return Err(MinimalLifeCompositionError::PolicyMismatch);
    }
    if facts.unexpired_entries() != 0 {
        return Err(MinimalLifeCompositionError::UnexpiredEvents);
    }
    if !facts.is_unregistered_and_empty() {
        return Err(MinimalLifeCompositionError::ExistingFacts);
    }
    Ok(facts.generation())
}
impl Evidence<'_, '_> {
    fn preflight(
        &self,
        m: MinimalLifeMembers,
        p: MinimalLifeParameters,
    ) -> Result<[u64; 4], MinimalLifeCompositionError> {
        self.movement.validate_writer_declaration(m.intent)?;
        self.movement.validate_reader_declaration(m.execution)?;
        self.motion.validate_writer_declaration(m.execution)?;
        if self.movement.state_policy(m.life)? != StatePolicy::initialized_only()
            || self.motion.state_policy(m.life)? != StatePolicy::initialized_only()
        {
            return Err(MinimalLifeCompositionError::PolicyMismatch);
        }
        if self.movement.has_slot(m.life)? || self.motion.has_slot(m.life)? {
            return Err(MinimalLifeCompositionError::ExistingFacts);
        }
        Ok([
            pristine_event(
                &self.gaze,
                m.life,
                m.intent,
                Some(m.execution),
                p.0.gaze.event_policy(),
            )?,
            pristine_event(
                &self.jump,
                m.life,
                m.intent,
                Some(m.execution),
                p.0.jump.event_policy(),
            )?,
            pristine_event(
                &self.command,
                m.life,
                m.intent,
                Some(m.execution),
                p.0.interaction.command_policy(),
            )?,
            pristine_event(
                &self.experience,
                m.life,
                m.execution,
                None,
                p.0.interaction.experience_policy(),
            )?,
        ])
    }
    pub(super) fn current(&self, entry: &Application) -> Result<(), MinimalLifeCompositionError> {
        let m = entry.members;
        let p = entry.parameters;
        self.movement.validate_writer(m.life, m.intent)?;
        self.movement.validate_reader(m.life, m.execution)?;
        self.motion.validate_writer(m.life, m.execution)?;
        self.gaze.validate_writer(m.life, m.intent)?;
        self.gaze.validate_reader(m.life, m.execution)?;
        self.jump.validate_writer(m.life, m.intent)?;
        self.jump.validate_reader(m.life, m.execution)?;
        self.command.validate_writer(m.life, m.intent)?;
        self.command.validate_reader(m.life, m.execution)?;
        self.experience.validate_writer(m.life, m.execution)?;
        let states = [
            self.movement.slot_evidence(m.life)?,
            self.motion.slot_evidence(m.life)?,
        ];
        if states.map(|value| value.map(StateSlotEvidence::generation)) != entry.slots {
            return Err(MinimalLifeCompositionError::ExistingFacts);
        }
        let events = [
            self.gaze.interface_evidence(m.life)?,
            self.jump.interface_evidence(m.life)?,
            self.command.interface_evidence(m.life)?,
            self.experience.interface_evidence(m.life)?,
        ];
        if events.map(EventInterfaceEvidence::generation) != entry.interfaces
            || events.map(EventInterfaceEvidence::policy)
                != [
                    p.0.gaze.event_policy(),
                    p.0.jump.event_policy(),
                    p.0.interaction.command_policy(),
                    p.0.interaction.experience_policy(),
                ]
        {
            return Err(MinimalLifeCompositionError::PolicyMismatch);
        }
        Ok(())
    }
}
pub(super) struct Prepared {
    members: MinimalLifeMembers,
    parameters: MinimalLifeParameters,
    coordinate: CoordinateIncarnation,
    region: PhysicsDemandRegion,
    baseline: CreatureMotionExperienceState,
    interfaces: [u64; 4],
}
#[allow(clippy::type_complexity)]
pub(super) fn preflight(
    In((members, parameters, coordinate, region)): In<(
        MinimalLifeMembers,
        MinimalLifeParameters,
        CoordinateIncarnation,
        PhysicsDemandRegion,
    )>,
    instances: LifeInstances,
    tick: Res<AuthoritativeFixedTick>,
    apps: Res<Applications>,
    assemblies: Res<CreatureAssemblyRegistry>,
    roles: Query<(
        Option<&IntentBodyId>,
        Option<&ExecutionBodyId>,
        Option<&RecordBodyId>,
    )>,
    facts: Query<(
        Option<&FactoryMotionRoute>,
        Option<&ApplicationOwner>,
        Option<&CreatureMotionRecord>,
        Option<&BlockInteractionRecord>,
    )>,
    executions: Query<(
        &Transform,
        &BodyOrientation,
        &GazeOrientation,
        &KinematicVelocity,
        &LocomotionState,
        &AabbDimensions,
        &JumpInputMode,
        &FactoryMotionPolicy,
        &FactoryMotionReadiness,
        &EnvironmentSample,
    )>,
    evidence: Evidence,
    world: LocalPhysicsDemandFacts,
) -> Result<Prepared, MinimalLifeCompositionError> {
    instances.resolve_life(members.life)?;
    if members.intent == members.execution
        || members.intent == members.record
        || members.execution == members.record
    {
        return Err(MinimalLifeCompositionError::InvalidMembers);
    }
    let [intent, execution, record] = members.resolve(&instances)?;
    for (entity, role) in [(intent, 0), (execution, 1), (record, 2)] {
        let tags = roles
            .get(entity)
            .map_err(|_| MinimalLifeCompositionError::InvalidMembers)?;
        if [tags.0.is_some(), tags.1.is_some(), tags.2.is_some()]
            != [role == 0, role == 1, role == 2]
        {
            return Err(MinimalLifeCompositionError::InvalidMembers);
        }
        let values = facts
            .get(entity)
            .map_err(|_| MinimalLifeCompositionError::InvalidMembers)?;
        if values.0.is_some() || values.1.is_some() || values.2.is_some() || values.3.is_some() {
            return Err(MinimalLifeCompositionError::ExistingFacts);
        }
    }
    if assemblies.get(members.life).is_some()
        || members
            .bodies()
            .iter()
            .any(|body| assemblies.life_for_body(*body).is_some())
        || apps.entries.values().any(|entry| {
            entry.members.life == members.life
                || entry
                    .members
                    .bodies()
                    .iter()
                    .any(|body| members.bodies().contains(body))
        })
    {
        return Err(MinimalLifeCompositionError::Duplicate);
    }
    if apps.entries.len() >= apps.capacity {
        return Err(MinimalLifeCompositionError::Capacity);
    }
    let (transform, body, gaze, velocity, locomotion, aabb, mode, policy, gate, environment) =
        executions
            .get(execution)
            .map_err(|_| MinimalLifeCompositionError::InvalidExecutionPolicy)?;
    if *mode != JumpInputMode::Discrete
        || *policy != FactoryMotionPolicy::RequireReadiness
        || !gate.is_blocked()
        || *aabb != parameters.0.viewpoint.aabb()
        || transform.scale != Vec3::ONE
        || transform.rotation != Quat::IDENTITY
    {
        return Err(MinimalLifeCompositionError::InvalidExecutionPolicy);
    }
    EnvironmentSample::new(
        environment.gravity,
        environment.static_friction,
        environment.kinetic_friction,
        environment.revision,
    )?;
    let baseline = CreatureMotionExperienceState::new(
        *tick,
        transform.translation.to_array(),
        body.0.to_array(),
        gaze.0.to_array(),
        velocity.0.to_array(),
        *locomotion,
    )?;
    let interfaces = evidence.preflight(members, parameters)?;
    world.validate_coordinate(coordinate)?;
    world
        .budgets()?
        .physics_demands()
        .region(region.minimum(), region.maximum())
        .map_err(|_| MinimalLifeCompositionError::World(WorldInteractionError::InvalidRegion))?;
    Ok(Prepared {
        members,
        parameters,
        coordinate,
        region,
        baseline,
        interfaces,
    })
}
pub(super) fn begin_application(
    In(prepared): In<Prepared>,
    mut apps: ResMut<Applications>,
    mut ids: InteractionAttemptIds,
) -> Result<InteractionActivationId, MinimalLifeCompositionError> {
    if apps.entries.len() >= apps.capacity {
        return Err(MinimalLifeCompositionError::Capacity);
    }
    let attempts = ids.begin_activation()?;
    let id = attempts.activation();
    apps.entries.insert(
        id,
        Application {
            members: prepared.members,
            parameters: prepared.parameters,
            coordinate: prepared.coordinate,
            region: prepared.region,
            status: MinimalLifeStatus::Closing,
            registrations: 0,
            slots: [None; 2],
            pending: [None; 2],
            interfaces: prepared.interfaces,
            demand: None,
            baseline: prepared.baseline,
            gaze_total: [0.0; 2],
        },
    );
    Ok(id)
}
type Managers<'w, 's> = ParamSet<
    'w,
    's,
    (
        LifeStateManagement<'static, 'static, MovementCommandState>,
        LifeStateManagement<'static, 'static, CreatureMotionExperienceState>,
        LifeEventManagement<'static, 'static, GazeCommandEvent>,
        LifeEventManagement<'static, 'static, JumpCommandEvent>,
        LifeEventManagement<'static, 'static, BlockInteractionCommandEvent>,
        LifeEventManagement<'static, 'static, BlockInteractionExperienceEvent>,
        Evidence<'static, 'static>,
    ),
>;
pub(super) fn approve(
    In(id): In<InteractionActivationId>,
    apps: Res<Applications>,
    mut managers: Managers,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let m = entry.members;
    macro_rules! observe {
        ($stage:ident) => {};
    }
    managers.p0().approve_origin(m.life, m.intent)?;
    observe!(ApproveMovementOrigin);
    managers.p0().approve_reader(m.life, m.execution)?;
    observe!(ApproveMovementReader);
    managers.p1().approve_origin(m.life, m.execution)?;
    observe!(ApproveMotionOrigin);
    // Historical fault checkpoints now observe the empty Recorder policy;
    // they no longer approve or register a Recorder reader.
    observe!(ApproveMotionReader);
    macro_rules! event {
        ($index:ident, $writer:expr, $reader:expr, $writer_stage:ident, $reader_stage:ident) => {{
            managers.$index().approve_writer(m.life, $writer)?;
            observe!($writer_stage);
            managers.$index().approve_reader(m.life, $reader)?;
            observe!($reader_stage);
        }};
    }
    event!(
        p2,
        m.intent,
        m.execution,
        ApproveGazeWriter,
        ApproveGazeReader
    );
    event!(
        p3,
        m.intent,
        m.execution,
        ApproveJumpWriter,
        ApproveJumpReader
    );
    event!(
        p4,
        m.intent,
        m.execution,
        ApproveCommandWriter,
        ApproveCommandReader
    );
    managers.p5().approve_writer(m.life, m.execution)?;
    observe!(ApproveExperienceWriter);
    observe!(ApproveExperienceReader);
    Ok(())
}
pub(super) fn note_slot(
    In((id, index)): In<(InteractionActivationId, usize)>,
    mut apps: ResMut<Applications>,
    evidence: Evidence,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get_mut(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let facts = if index == 0 {
        evidence.movement.slot_evidence(entry.members.life)?
    } else {
        evidence.motion.slot_evidence(entry.members.life)?
    }
    .ok_or(MinimalLifeCompositionError::State(StateError::SlotNotFound))?;
    let origin = if index == 0 {
        entry.members.intent
    } else {
        entry.members.execution
    };
    // The structural completion owns this stable generation even if an
    // invalidation froze its writer before this native consumer runs.
    entry.slots[index] = Some(facts.generation());
    entry.registrations |= 1 << (index * 2);
    if facts.origin() != origin || !facts.has_writer() {
        return Err(MinimalLifeCompositionError::InvalidMembers);
    }
    Ok(())
}
pub(super) fn register(
    In(id): In<InteractionActivationId>,
    mut apps: ResMut<Applications>,
    mut managers: Managers,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get_mut(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let m = entry.members;
    managers.p0().register_reader(m.life, m.execution)?;
    entry.registrations |= 1 << 1;
    macro_rules! event {
        ($index:ident, $writer:expr, $reader:expr, $bit:expr, $writer_stage:ident, $reader_stage:ident) => {{
            {
                let mut p = managers.$index();
                p.register_writer(m.life, $writer)?;
                entry.registrations |= 1 << $bit;
                p.register_reader(m.life, $reader)?;
                entry.registrations |= 1 << ($bit + 1);
            }
        }};
    }
    managers.p2().register_writer(m.life, m.intent)?;
    entry.registrations |= 1 << 4;
    managers.p2().register_reader(m.life, m.execution)?;
    entry.registrations |= 1 << 5;
    event!(p3, m.intent, m.execution, 6, JumpWriter, JumpReader);
    event!(p4, m.intent, m.execution, 8, CommandWriter, CommandReader);
    managers.p5().register_writer(m.life, m.execution)?;
    entry.registrations |= 1 << 10;
    Ok(())
}
pub(super) fn install_demand(
    In(id): In<InteractionActivationId>,
    instances: LifeInstances,
    mut apps: ResMut<Applications>,
    mut demands: LocalPhysicsDemands,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get_mut(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    entry.members.resolve(&instances)?;
    entry.demand = Some(demands.register(entry.coordinate, entry.region)?);
    Ok(())
}
pub(super) fn install_owner(
    In((id, index)): In<(InteractionActivationId, usize)>,
    instances: LifeInstances,
    apps: Res<Applications>,
    mut commands: Commands,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let entity = instances.resolve_body(entry.members.bodies()[index])?;
    commands.entity(entity).insert(ApplicationOwner(id));
    Ok(())
}
pub(super) fn install_route(
    In(id): In<InteractionActivationId>,
    instances: LifeInstances,
    apps: Res<Applications>,
    mut commands: Commands,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let intent = instances.resolve_body(entry.members.intent)?;
    commands.entity(intent).insert(entry.members.motion_route());
    Ok(())
}
/// This fixed recipe has an empty Recorder: no reader declaration is required,
/// no reader is registered and no recording policy is installed.
pub(super) fn check_motion_recorder(
    In(id): In<InteractionActivationId>,
    instances: LifeInstances,
    apps: Res<Applications>,
    records: Query<&CreatureMotionRecord>,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let record = instances.resolve_body(entry.members.record)?;
    if records.contains(record) {
        return Err(MinimalLifeCompositionError::ExistingFacts);
    }
    Ok(())
}
pub(super) fn check_interaction_recorder(
    In(id): In<InteractionActivationId>,
    instances: LifeInstances,
    apps: Res<Applications>,
    records: Query<&BlockInteractionRecord>,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    let record = instances.resolve_body(entry.members.record)?;
    if records.contains(record) {
        return Err(MinimalLifeCompositionError::ExistingFacts);
    }
    Ok(())
}
pub(super) fn publish(
    In(id): In<InteractionActivationId>,
    instances: LifeInstances,
    mut apps: ResMut<Applications>,
    mut assemblies: ResMut<CreatureAssemblyRegistry>,
    evidence: Evidence,
    demands: LocalPhysicsDemandFacts,
    owners: Query<&ApplicationOwner>,
    routes: Query<&FactoryMotionRoute>,
    records: Query<(
        Option<&CreatureMotionRecord>,
        Option<&BlockInteractionRecord>,
    )>,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = apps
        .entries
        .get(&id)
        .ok_or(MinimalLifeCompositionError::Inactive)?;
    instances.resolve_life(entry.members.life)?;
    evidence.current(entry)?;
    demands.validate_coordinate(entry.coordinate)?;
    let [intent, _, record] = entry.members.resolve(&instances)?;
    for body in entry.members.bodies() {
        if owners
            .get(instances.resolve_body(body)?)
            .map(|owner| owner.0)
            .ok()
            != Some(id)
        {
            return Err(MinimalLifeCompositionError::ExistingFacts);
        }
    }
    if routes.get(intent).ok() != Some(&entry.members.motion_route()) {
        return Err(MinimalLifeCompositionError::InvalidMembers);
    }
    let (motion, interaction) = records
        .get(record)
        .map_err(|_| MinimalLifeCompositionError::ExistingFacts)?;
    if motion.is_some() || interaction.is_some() {
        return Err(MinimalLifeCompositionError::ExistingFacts);
    }
    assemblies
        .insert_minimal(entry.members.life, entry.members.members())
        .map_err(|_| MinimalLifeCompositionError::Duplicate)?;
    apps.entries.get_mut(&id).unwrap().status = MinimalLifeStatus::Active;
    Ok(())
}

/// Ordinary targeted cleanup; structural Slot completion stays in the Life kernel.
#[derive(SystemParam)]
pub(super) struct Cleanup<'w, 's> {
    apps: ResMut<'w, Applications>,
    instances: LifeInstances<'w, 's>,
    assemblies: ResMut<'w, CreatureAssemblyRegistry>,
    managers: Managers<'w, 's>,
    motion: FactoryMotionLifecycle<'w, 's>,
    demands: LocalPhysicsDemands<'w, 's>,
    commands: Commands<'w, 's>,
    owners: Query<'w, 's, &'static ApplicationOwner>,
    gates: Query<'w, 's, &'static mut FactoryMotionReadiness>,
}
impl Cleanup<'_, '_> {
    pub(super) fn affected(
        &self,
        invalidation: FactoryInvalidation,
    ) -> Vec<InteractionActivationId> {
        let actually_invalid = match invalidation {
            FactoryInvalidation::Life(life) => self.instances.resolve_life(life).is_err(),
            FactoryInvalidation::ConceptBody(body) => self.instances.resolve_body(body).is_err(),
        };
        if !actually_invalid {
            return Vec::new();
        }
        self.apps
            .entries
            .iter()
            .filter_map(|(&id, entry)| match invalidation {
                FactoryInvalidation::Life(life) => (entry.members.life == life).then_some(id),
                FactoryInvalidation::ConceptBody(body) => {
                    entry.members.bodies().contains(&body).then_some(id)
                }
            })
            .collect()
    }
    pub(super) fn close(&mut self, id: InteractionActivationId) {
        let Applications {
            entries, closing, ..
        } = &mut *self.apps;
        let Some(entry) = entries.get_mut(&id) else {
            return;
        };
        if !matches!(entry.status, MinimalLifeStatus::CleanupBlocked(_)) {
            entry.status = MinimalLifeStatus::Closing;
        }
        if !closing.contains(&id) {
            closing.push_back(id);
        }
        let m = entry.members;
        self.motion.clear_route(m.motion_route());
        if self.assemblies.get(m.life) == Some(m.members())
            && !self.assemblies.accepts_controller_input(m.life)
        {
            self.assemblies.remove_life(m.life);
        }
        if let Some(demand) = entry.demand.take() {
            let _ = self.demands.remove(demand);
        }
        for (index, body) in m.bodies().into_iter().enumerate() {
            if let Ok(entity) = self.instances.resolve_body(body)
                && self.owners.get(entity).is_ok_and(|owner| owner.0 == id)
            {
                if let Ok(mut gate) = self.gates.get_mut(entity) {
                    gate.block();
                }
                if index == 0 {
                    self.commands
                        .entity(entity)
                        .try_remove::<FactoryMotionRoute>();
                }
                self.commands
                    .entity(entity)
                    .try_remove::<ApplicationOwner>();
            }
        }
        let (states, events) = {
            let e = self.managers.p6();
            (
                [
                    e.movement
                        .slot_evidence(m.life)
                        .ok()
                        .flatten()
                        .map(StateSlotEvidence::generation),
                    e.motion
                        .slot_evidence(m.life)
                        .ok()
                        .flatten()
                        .map(StateSlotEvidence::generation),
                ],
                [
                    e.gaze
                        .interface_evidence(m.life)
                        .ok()
                        .map(EventInterfaceEvidence::generation),
                    e.jump
                        .interface_evidence(m.life)
                        .ok()
                        .map(EventInterfaceEvidence::generation),
                    e.command
                        .interface_evidence(m.life)
                        .ok()
                        .map(EventInterfaceEvidence::generation),
                    e.experience
                        .interface_evidence(m.life)
                        .ok()
                        .map(EventInterfaceEvidence::generation),
                ],
            )
        };
        if states[0] == entry.slots[0] && entry.slots[0].is_some() {
            let mut p = self.managers.p0();
            if entry.registrations & (1 << 1) != 0 {
                let _ = p.deregister_reader(m.life, m.execution);
            }
            if entry.registrations & 1 != 0 {
                let _ = p.deregister_writer(m.life, m.intent);
            }
        }
        if states[1] == entry.slots[1] && entry.slots[1].is_some() {
            let mut p = self.managers.p1();
            if entry.registrations & (1 << 3) != 0 {
                let _ = p.deregister_reader(m.life, m.record);
            }
            if entry.registrations & (1 << 2) != 0 {
                let _ = p.deregister_writer(m.life, m.execution);
            }
        }
        macro_rules! event {
            ($index:ident, $writer:expr, $reader:expr, $bit:expr, $generation:expr) => {{
                if events[$generation] == Some(entry.interfaces[$generation]) {
                    let mut p = self.managers.$index();
                    if entry.registrations & (1 << ($bit + 1)) != 0 {
                        let _ = p.deregister_reader(m.life, $reader);
                    }
                    if entry.registrations & (1 << $bit) != 0 {
                        let _ = p.deregister_writer(m.life, $writer);
                    }
                }
            }};
        }
        event!(p5, m.execution, m.record, 10, 3);
        event!(p4, m.intent, m.execution, 8, 2);
        event!(p3, m.intent, m.execution, 6, 1);
        event!(p2, m.intent, m.execution, 4, 0);
        entry.registrations = 0;
    }
}
