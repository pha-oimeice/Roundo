//! Fixed Controller-free composition of caller-owned Factory instances.
//! Native registration/data remain Life-owned; this ledger owns only work and cleanup.
mod intents;
mod registration;
pub(super) mod world_execution;
use super::{CreatureAssemblyMembers, CreatureAssemblyRegistry};
use bevy::{ecs::system::RunSystemOnce, prelude::*};
pub use intents::{MinimalLifeIntents, MinimalLifeSubmitError};
use roundo_block_interaction::*;
use roundo_creature::*;
use roundo_life::*;
use roundo_local_coordinate::*;
use std::collections::{HashMap, VecDeque};

/// Exact independent participants, not a lifecycle or data capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MinimalLifeMembers {
    pub life: LifeHandle,
    pub intent: ConceptBodyHandle,
    pub execution: ConceptBodyHandle,
    pub record: ConceptBodyHandle,
}
impl MinimalLifeMembers {
    fn bodies(self) -> [ConceptBodyHandle; 3] {
        [self.intent, self.execution, self.record]
    }
    fn resolve(self, instances: &LifeInstances) -> Result<[Entity; 3], FactoryError> {
        Ok([
            instances.resolve_body(self.intent)?,
            instances.resolve_body(self.execution)?,
            instances.resolve_body(self.record)?,
        ])
    }
    fn members(self) -> CreatureAssemblyMembers {
        CreatureAssemblyMembers {
            intent: self.intent,
            execution: self.execution,
            record: self.record,
        }
    }
    pub fn motion_route(self) -> FactoryMotionRoute {
        FactoryMotionRoute::new(self.life, self.intent, self.execution, self.record)
    }
}
/// All choices are explicit. World work budgets are supplied separately to the Plugin.
#[derive(Clone, Copy, Debug)]
pub struct MinimalLifeParameterValues {
    pub gaze: OneTickCommandBudget<GazeCommandEvent>,
    pub jump: OneTickCommandBudget<JumpCommandEvent>,
    pub interaction: BlockInteractionBudgets,
    pub viewpoint: CreatureViewpoint,
    pub edit_permission: bool,
    pub maximum_sweep_distance: f32,
    pub neighbor_chunks: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct MinimalLifeParameters(MinimalLifeParameterValues);
impl MinimalLifeParameters {
    pub fn new(values: MinimalLifeParameterValues) -> Result<Self, MinimalLifeCompositionError> {
        if !values.maximum_sweep_distance.is_finite()
            || values.maximum_sweep_distance <= 0.0
            || values.neighbor_chunks == 0
            || values.neighbor_chunks > i32::MAX as usize
        {
            return Err(MinimalLifeCompositionError::InvalidParameter);
        }
        Ok(Self(values))
    }
    pub const fn values(self) -> MinimalLifeParameterValues {
        self.0
    }
    /// Prepares declarations/policies only; never creates or composes instances.
    pub fn life_prototype(self) -> LifePrototype {
        LifePrototype::empty()
            .with_state_policy::<MovementCommandState>(StatePolicy::initialized_only())
            .with_state_policy::<CreatureMotionExperienceState>(StatePolicy::initialized_only())
            .with_event_interface::<GazeCommandEvent>(self.0.gaze.event_policy())
            .with_event_interface::<JumpCommandEvent>(self.0.jump.event_policy())
            .with_event_interface::<BlockInteractionCommandEvent>(
                self.0.interaction.command_policy(),
            )
            .with_event_interface::<BlockInteractionExperienceEvent>(
                self.0.interaction.experience_policy(),
            )
    }
    pub fn intent_prototype<B: Bundle>(self, bundle: B) -> ConceptBodyPrototype<B> {
        ConceptBodyPrototype::new(bundle)
            .with_state_writer::<MovementCommandState>()
            .with_event_writer::<GazeCommandEvent>()
            .with_event_writer::<JumpCommandEvent>()
            .with_event_writer::<BlockInteractionCommandEvent>()
    }
    /// The caller supplies the permanent Discrete/RequireReadiness policies and
    /// a blocked readiness gate in `bundle`; missing/other modes are rejected.
    pub fn execution_prototype<B: Bundle>(self, bundle: B) -> ConceptBodyPrototype<B> {
        ConceptBodyPrototype::new(bundle)
            .with_state_reader::<MovementCommandState>()
            .with_event_reader::<GazeCommandEvent>()
            .with_event_reader::<JumpCommandEvent>()
            .with_event_reader::<BlockInteractionCommandEvent>()
            .with_state_writer::<CreatureMotionExperienceState>()
            .with_event_writer::<BlockInteractionExperienceEvent>()
    }
    pub fn record_prototype<B: Bundle>(self, bundle: B) -> ConceptBodyPrototype<B> {
        ConceptBodyPrototype::new(bundle)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinimalCleanupError {
    Submit(FactorySubmitError),
    Panicked,
    State(StateError),
    Protocol,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinimalLifeStatus {
    Active,
    Closing,
    CleanupBlocked(MinimalCleanupError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinimalLifeCompositionError {
    InvalidParameter,
    InvalidMembers,
    InvalidExecutionPolicy,
    Duplicate,
    PolicyMismatch,
    ExistingFacts,
    UnexpiredEvents,
    Capacity,
    Inactive,
    CleanupPending,
    Factory(FactoryError),
    State(StateError),
    Event(EventError),
    Motion(CreatureMotionError),
    Scoped(ScopedLifeError),
    World(WorldInteractionError),
    Id(InteractionIdError),
    CleanupBlocked(MinimalCleanupError),
}
macro_rules! from_error {
    ($variant:ident, $ty:ty) => {
        impl From<$ty> for MinimalLifeCompositionError {
            fn from(error: $ty) -> Self {
                Self::$variant(error)
            }
        }
    };
}
from_error!(Factory, FactoryError);
from_error!(State, StateError);
from_error!(Event, EventError);
from_error!(Motion, CreatureMotionError);
from_error!(Scoped, ScopedLifeError);
from_error!(World, WorldInteractionError);
from_error!(Id, InteractionIdError);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum MinimalLifeApplicationSet {
    Management,
    BeginIntents,
    Intents,
    EndIntents,
}
/// Rules only. Application capacity includes unpublished/Closing/Blocked work;
/// it is not released merely because a route or caller instance disappears.
pub struct MinimalLifeApplicationPlugin {
    capacity: usize,
    world: WorldInteractionRuntimeBudgets,
}
impl MinimalLifeApplicationPlugin {
    pub fn new(
        capacity: usize,
        world: WorldInteractionRuntimeBudgets,
    ) -> Result<Self, MinimalLifeCompositionError> {
        if capacity == 0 || capacity > world.physics_demands().demands() {
            return Err(MinimalLifeCompositionError::InvalidParameter);
        }
        Ok(Self { capacity, world })
    }
}
impl Plugin for MinimalLifeApplicationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<CreatureLifeDataPlugin>() {
            app.add_plugins(CreatureLifeDataPlugin);
        }
        if !app.is_plugin_added::<BlockInteractionLifeDataPlugin>() {
            app.add_plugins(BlockInteractionLifeDataPlugin);
        }
        if !app.is_plugin_added::<LifeRuntimePlugin>() {
            app.add_plugins(LifeRuntimePlugin::default());
        }
        if !app.is_plugin_added::<WorldInteractionPlugin>() {
            app.add_plugins(WorldInteractionPlugin::new(self.world));
        }
        if !app.is_plugin_added::<super::AuthoritativeCreaturePlugin>() {
            install_factory_motion_route_validator::<CreatureAssemblyRegistry>(app)
                .expect("one shared Creature assembly validator");
        }
        if !app.is_plugin_added::<CreatureMotionPlugin>() {
            app.add_plugins(CreatureMotionPlugin);
        }
        app.init_resource::<VirtualChunkIndex>()
            .init_resource::<AtomicVoxelRegistry>()
            .init_resource::<CreatureAssemblyRegistry>()
            .insert_resource(Applications {
                capacity: self.capacity,
                entries: HashMap::new(),
                closing: VecDeque::new(),
            })
            .init_resource::<SubmissionWindow>()
            .configure_sets(
                FixedUpdate,
                (
                    MinimalLifeApplicationSet::Management,
                    MinimalLifeApplicationSet::BeginIntents,
                    MinimalLifeApplicationSet::Intents,
                    MinimalLifeApplicationSet::EndIntents,
                )
                    .chain()
                    .after(LifeFixedSet::CommitFactories)
                    .before(CreatureLifeSet::CommandProjection),
            )
            .add_systems(
                FixedUpdate,
                (
                    settle_closing.in_set(MinimalLifeApplicationSet::Management),
                    begin_submission.in_set(MinimalLifeApplicationSet::BeginIntents),
                    end_submission.in_set(MinimalLifeApplicationSet::EndIntents),
                    world_execution::refresh_readiness
                        .after(CreatureLifeSet::CommandProjection)
                        .before(CreatureLifeSet::MotionCommit),
                    world_execution::execute_interactions
                        .in_set(VoxelEditSet::Commit)
                        .after(CreatureLifeSet::MotionCommit),
                ),
            )
            .add_observer(close_invalidated_application);
    }
}
#[derive(Component, Clone, Copy)]
struct ApplicationOwner(InteractionActivationId);
struct Application {
    members: MinimalLifeMembers,
    parameters: MinimalLifeParameters,
    coordinate: CoordinateIncarnation,
    region: PhysicsDemandRegion,
    status: MinimalLifeStatus,
    registrations: u16,
    slots: [Option<u64>; 2],
    pending: [Option<LifeStructureRequestId>; 2],
    interfaces: [u64; 4],
    demand: Option<PhysicsDemandId>,
    baseline: CreatureMotionExperienceState,
    gaze_total: [f32; 2],
}
#[derive(Resource)]
struct Applications {
    capacity: usize,
    entries: HashMap<InteractionActivationId, Application>,
    closing: VecDeque<InteractionActivationId>,
}
#[derive(Resource, Default)]
struct SubmissionWindow(Option<AuthoritativeFixedTick>);

/// Copies lifecycle status without granting registration, removal or data access.
pub fn minimal_life_status(
    world: &World,
    id: InteractionActivationId,
) -> Option<MinimalLifeStatus> {
    world
        .get_resource::<Applications>()?
        .entries
        .get(&id)
        .map(|entry| entry.status)
}
fn begin_submission(
    tick: Res<AuthoritativeFixedTick>,
    mut window: ResMut<SubmissionWindow>,
    mut apps: ResMut<Applications>,
) {
    window.0 = Some(*tick);
    for entry in apps.entries.values_mut() {
        entry.gaze_total = [0.0; 2];
    }
}
fn end_submission(mut window: ResMut<SubmissionWindow>) {
    window.0 = None;
}
fn begin_scope(mut m: LifeScopedManagement) -> Result<LifeCommitScope, ScopedLifeError> {
    m.begin_scope()
}
fn finish_scope(
    In(scope): In<LifeCommitScope>,
    mut m: LifeScopedManagement,
) -> Result<(), ScopedLifeError> {
    m.finish_scope(scope)
}
fn take_scoped(
    In((scope, id)): In<(LifeCommitScope, ScopedLifeRequestId)>,
    mut m: LifeScopedManagement,
) -> Option<ScopedLifeCompletion> {
    m.take_completion(scope, id)
}
fn commit_exact(
    world: &mut World,
    scope: LifeCommitScope,
    id: ScopedLifeRequestId,
) -> Result<(), MinimalLifeCompositionError> {
    let committed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_scoped_life_request(world, scope, id)
    }));
    let completion = world
        .run_system_once_with(take_scoped, (scope, id))
        .expect("native scoped result parameter");
    match committed {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(result) => {
            result?;
            match completion {
                Some(ScopedLifeCompletion::State(result)) => Ok(result?),
                _ => Err(MinimalLifeCompositionError::CleanupBlocked(
                    MinimalCleanupError::Protocol,
                )),
            }
        }
    }
}
fn request_slot<S: StateData>(
    In((scope, life, origin, initial)): In<(LifeCommitScope, LifeHandle, ConceptBodyHandle, S)>,
    mut m: LifeScopedManagement,
) -> Result<ScopedLifeRequestId, ScopedLifeError> {
    m.create_state_slot(scope, life, origin, Some(initial))
}
fn create_slot<S: StateData>(
    world: &mut World,
    scope: LifeCommitScope,
    life: LifeHandle,
    origin: ConceptBodyHandle,
    initial: S,
) -> Result<(), MinimalLifeCompositionError> {
    let id = world
        .run_system_once_with(request_slot::<S>, (scope, life, origin, initial))
        .expect("native Slot request parameter")?;
    commit_exact(world, scope, id)
}
fn remove_slot<S: StateData>(
    world: &mut World,
    scope: LifeCommitScope,
    life: LifeHandle,
    generation: u64,
) -> Result<(), MinimalLifeCompositionError> {
    let id = world
        .run_system_once(move |mut m: LifeScopedManagement| {
            m.remove_state_slot_if_generation::<S>(scope, life, generation)
        })
        .expect("native conditional Slot request parameter")?;
    match commit_exact(world, scope, id) {
        Err(MinimalLifeCompositionError::State(
            StateError::LifeNotFound | StateError::SlotNotFound | StateError::SlotChanged,
        )) => Ok(()),
        result => result,
    }
}
/// Explicit serialized host seam. All four instances are caller-owned; this
/// function never calls a Factory create/remove or accepts a World callback.
/// Success follows real scoped completions and complete current registration.
pub fn compose_minimal_life(
    world: &mut World,
    members: MinimalLifeMembers,
    parameters: MinimalLifeParameters,
    coordinate: CoordinateIncarnation,
    region: PhysicsDemandRegion,
) -> Result<InteractionActivationId, MinimalLifeCompositionError> {
    let prepared = world
        .run_system_once_with(
            registration::preflight,
            (members, parameters, coordinate, region),
        )
        .expect("minimal composition preflight parameters")?;
    let scope = world
        .run_system_once(begin_scope)
        .expect("scoped lifecycle installed")?;
    let begun = world
        .run_system_once_with(registration::begin_application, prepared)
        .expect("minimal application ledger and source IDs installed");
    let id = match begun {
        Ok(id) => id,
        Err(error) => {
            world
                .run_system_once_with(finish_scope, scope)
                .expect("scope finish parameter")?;
            return Err(error);
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world
            .run_system_once_with(registration::approve, id)
            .expect("native typed approvals")?;
        macro_rules! observe {
            ($stage:ident) => {};
        }
        let baseline = world.resource::<Applications>().entries[&id].baseline;
        create_slot(
            world,
            scope,
            members.life,
            members.intent,
            MovementCommandState::default(),
        )?;
        world
            .run_system_once_with(registration::note_slot, (id, 0))
            .expect("native structural evidence")?;
        observe!(MovementSlotCompleted);
        create_slot(world, scope, members.life, members.execution, baseline)?;
        world
            .run_system_once_with(registration::note_slot, (id, 1))
            .expect("native structural evidence")?;
        observe!(MotionSlotCompleted);
        world
            .run_system_once_with(registration::register, id)
            .expect("native typed registrations")?;
        world
            .run_system_once_with(registration::install_demand, id)
            .expect("targeted recipe demand")?;
        observe!(DemandInstalled);
        world
            .run_system_once_with(registration::install_owner, (id, 0))
            .expect("targeted Intent owner")?;
        observe!(IntentOwnerInstalled);
        world
            .run_system_once_with(registration::install_owner, (id, 1))
            .expect("targeted Execution owner")?;
        observe!(ExecutionOwnerInstalled);
        world
            .run_system_once_with(registration::install_owner, (id, 2))
            .expect("targeted Record owner")?;
        observe!(RecordOwnerInstalled);
        world
            .run_system_once_with(registration::install_route, id)
            .expect("targeted Intent route")?;
        observe!(RouteInstalled);
        world
            .run_system_once_with(registration::check_motion_recorder, id)
            .expect("empty motion Recorder")?;
        observe!(MotionRecorderChecked);
        world
            .run_system_once_with(registration::check_interaction_recorder, id)
            .expect("empty interaction Recorder")?;
        observe!(InteractionRecorderChecked);
        world
            .run_system_once_with(registration::publish, id)
            .expect("complete recipe revalidation")?;
        Ok(id)
    }));
    if matches!(&result, Ok(Ok(_))) {
        world
            .run_system_once_with(finish_scope, scope)
            .expect("scope finish parameter")
            .expect("every scoped completion was consumed");
        return result.unwrap();
    }
    let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world
            .run_system_once_with(close_application, id)
            .expect("targeted compensation parameters");
        finish_owned_slots(world, scope, id)
    }));
    match &cleanup {
        Ok(Err(error)) => mark_cleanup_blocked(world, id, cleanup_error_kind(*error)),
        Err(_) => mark_cleanup_blocked(world, id, MinimalCleanupError::Panicked),
        Ok(Ok(())) => {}
    }
    // Even compensation unwind must release a fully consumed native scope.
    // Do not let a secondary protocol fault replace the original panic payload.
    let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match world.run_system_once_with(finish_scope, scope) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(MinimalCleanupError::Protocol),
        }
    }));
    match &finished {
        Ok(Err(error)) => mark_cleanup_blocked(world, id, *error),
        Err(_) => mark_cleanup_blocked(world, id, MinimalCleanupError::Panicked),
        Ok(Ok(())) => {}
    }
    let original_error = match result {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(Err(error)) => error,
        Ok(Ok(_)) => unreachable!("successful composition returned before compensation"),
    };
    let cleanup_error = match cleanup {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(result) => result.err(),
    };
    match finished {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(Err(error)) => Err(MinimalLifeCompositionError::CleanupBlocked(error)),
        Ok(Ok(())) => Err(cleanup_error.map_or(original_error, |error| {
            MinimalLifeCompositionError::CleanupBlocked(cleanup_error_kind(error))
        })),
    }
}
fn finish_owned_slots(
    world: &mut World,
    scope: LifeCommitScope,
    id: InteractionActivationId,
) -> Result<(), MinimalLifeCompositionError> {
    let entry = &world.resource::<Applications>().entries[&id];
    let (life, slots) = (entry.members.life, entry.slots);
    if let Some(generation) = slots[1] {
        remove_slot::<CreatureMotionExperienceState>(world, scope, life, generation)?;
        world
            .resource_mut::<Applications>()
            .entries
            .get_mut(&id)
            .unwrap()
            .slots[1] = None;
    }
    if let Some(generation) = slots[0] {
        remove_slot::<MovementCommandState>(world, scope, life, generation)?;
        world
            .resource_mut::<Applications>()
            .entries
            .get_mut(&id)
            .unwrap()
            .slots[0] = None;
    }
    let mut apps = world.resource_mut::<Applications>();
    apps.entries.remove(&id);
    apps.closing.retain(|candidate| *candidate != id);
    Ok(())
}
/// Closes submission immediately. Success includes precise Slot cleanup; a busy
/// scope can return an error with status Closing, completed by ordinary native
/// maintenance. Caller identities and prototype interfaces/policies survive.
pub fn unload_minimal_life(
    world: &mut World,
    id: InteractionActivationId,
) -> Result<(), MinimalLifeCompositionError> {
    let Some(entry) = world
        .get_resource::<Applications>()
        .and_then(|apps| apps.entries.get(&id))
    else {
        return Err(MinimalLifeCompositionError::Inactive);
    };
    if let MinimalLifeStatus::CleanupBlocked(error) = entry.status {
        return Err(MinimalLifeCompositionError::CleanupBlocked(error));
    }
    if entry.pending.iter().any(Option::is_some) {
        return Err(MinimalLifeCompositionError::CleanupPending);
    }
    world
        .run_system_once_with(close_application, id)
        .expect("targeted unload parameters");
    let scope = match world
        .run_system_once(begin_scope)
        .expect("scoped lifecycle installed")
    {
        Ok(scope) => scope,
        Err(error) => {
            if let ScopedLifeError::Submit(FactorySubmitError::IdExhausted) = error {
                mark_cleanup_blocked(
                    world,
                    id,
                    MinimalCleanupError::Submit(FactorySubmitError::IdExhausted),
                );
            }
            return Err(error.into());
        }
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_owned_slots(world, scope, id)
    }));
    match &result {
        Ok(Err(error)) => mark_cleanup_blocked(world, id, cleanup_error_kind(*error)),
        Err(_) => mark_cleanup_blocked(world, id, MinimalCleanupError::Panicked),
        Ok(Ok(())) => {}
    }
    // Always finalize independently; a secondary failure must not swallow the
    // primary cleanup panic or cause unsafe resubmission by maintenance.
    let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match world.run_system_once_with(finish_scope, scope) {
            Ok(result) => result.map_err(|_| MinimalCleanupError::Protocol),
            Err(_) => Err(MinimalCleanupError::Protocol),
        }
    }));
    match &finished {
        Ok(Err(error)) => mark_cleanup_blocked(world, id, *error),
        Err(_) => mark_cleanup_blocked(world, id, MinimalCleanupError::Panicked),
        Ok(Ok(())) => {}
    }
    let cleanup = match result {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(result) => result,
    };
    match finished {
        Err(payload) => std::panic::resume_unwind(payload),
        Ok(Err(error)) => Err(MinimalLifeCompositionError::CleanupBlocked(error)),
        Ok(Ok(())) => cleanup,
    }
}
fn cleanup_error_kind(error: MinimalLifeCompositionError) -> MinimalCleanupError {
    match error {
        MinimalLifeCompositionError::Scoped(ScopedLifeError::Submit(error)) => {
            MinimalCleanupError::Submit(error)
        }
        MinimalLifeCompositionError::State(error) => MinimalCleanupError::State(error),
        MinimalLifeCompositionError::CleanupBlocked(error) => error,
        _ => MinimalCleanupError::Protocol,
    }
}
fn mark_cleanup_blocked(
    world: &mut World,
    id: InteractionActivationId,
    error: MinimalCleanupError,
) {
    if let Some(entry) = world.resource_mut::<Applications>().entries.get_mut(&id) {
        entry.status = MinimalLifeStatus::CleanupBlocked(error);
    }
}

fn close_application(In(id): In<InteractionActivationId>, mut cleanup: registration::Cleanup) {
    cleanup.close(id);
}
fn close_invalidated_application(
    event: On<FactoryInvalidation>,
    mut cleanup: registration::Cleanup,
) {
    let ids = cleanup.affected(*event);
    for id in ids {
        cleanup.close(id);
    }
}
fn settle_closing(
    mut apps: ResMut<Applications>,
    instances: LifeInstances,
    mut structure: LifeStructureManagement,
) {
    let count = apps.closing.len();
    for _ in 0..count {
        let id = apps.closing.pop_front().unwrap();
        let Some(entry) = apps.entries.get_mut(&id) else {
            continue;
        };
        // Even Blocked work must claim already-submitted sibling completions;
        // stopping submission must not leak the shared native management budget.
        for index in 0..2 {
            if let Some(request) = entry.pending[index] {
                if let Some(completion) = structure.take_completion(request) {
                    entry.pending[index] = None;
                    match completion {
                        LifeStructureCompletion::State(
                            Ok(())
                            | Err(
                                StateError::LifeNotFound
                                | StateError::SlotNotFound
                                | StateError::SlotChanged,
                            ),
                        ) => entry.slots[index] = None,
                        LifeStructureCompletion::Panicked => {
                            entry.status =
                                MinimalLifeStatus::CleanupBlocked(MinimalCleanupError::Panicked)
                        }
                        LifeStructureCompletion::State(Err(error)) => {
                            entry.status =
                                MinimalLifeStatus::CleanupBlocked(MinimalCleanupError::State(error))
                        }
                        _ => {
                            entry.status =
                                MinimalLifeStatus::CleanupBlocked(MinimalCleanupError::Protocol)
                        }
                    }
                }
            }
            if entry.pending[index].is_none() && matches!(entry.status, MinimalLifeStatus::Closing)
            {
                if let Some(generation) = entry.slots[index] {
                    let submitted = if index == 0 {
                        structure.remove_state_slot_if_generation::<MovementCommandState>(
                            entry.members.life,
                            generation,
                        )
                    } else {
                        structure.remove_state_slot_if_generation::<CreatureMotionExperienceState>(
                            entry.members.life,
                            generation,
                        )
                    };
                    match submitted {
                        Ok(request) => {
                            entry.pending[index] = Some(request);
                        }
                        Err(FactorySubmitError::CapacityExceeded) => {}
                        Err(error) => {
                            entry.status = MinimalLifeStatus::CleanupBlocked(
                                MinimalCleanupError::Submit(error),
                            )
                        }
                    }
                }
            }
        }
        let settled = matches!(entry.status, MinimalLifeStatus::Closing)
            && entry.slots.iter().all(Option::is_none);
        let removed_life = instances.resolve_life(entry.members.life).is_err();
        if (settled || removed_life) && entry.pending.iter().all(Option::is_none) {
            apps.entries.remove(&id);
        } else {
            apps.closing.push_back(id);
        }
    }
}
