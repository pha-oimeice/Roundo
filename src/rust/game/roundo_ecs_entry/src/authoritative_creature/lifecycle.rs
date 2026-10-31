//! Exact bootstrap request/result steps, without assembly progress or rollback.
//! Producers and consumers are ordinary systems; commit helpers select only the
//! current scoped request and never drain the scheduled Factory queue.

use super::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::{Bundle, In, ResMut};
use roundo_controller::{
    ControllerCommitScope, ControllerLifecycleCompletion, ControllerLifecycleManagement,
    ControllerLifecycleRequestId, commit_scoped_controller_request,
};
use roundo_creature::{ExecutionBodyBundle, IntentBodyBundle, RecordBodyBundle};
use roundo_life::{
    FactoryCompletion, LifeCommitScope, LifeScopedManagement, ScopedLifeCompletion,
    ScopedLifeRequestId, StateData, commit_scoped_life_request,
};

pub(super) struct TestCreatureRecipe {
    pub(super) intent: ConceptBodyPrototype<IntentBodyBundle>,
    pub(super) execution: ConceptBodyPrototype<ExecutionBodyBundle>,
    pub(super) record: ConceptBodyPrototype<RecordBodyBundle>,
}

pub(super) fn begin_bootstrap_scopes(
    mut controllers: ControllerLifecycleManagement,
    mut life: LifeScopedManagement,
) -> Result<(LifeCommitScope, ControllerCommitScope), CreatureAssemblyError> {
    // Reserve both bounded lanes before creating any instance. A busy Controller
    // host cannot cause partial Life bootstrap or steal another host's result.
    let controller_scope = controllers.begin_scope()?;
    match life.begin_scope() {
        Ok(life_scope) => Ok((life_scope, controller_scope)),
        Err(error) => {
            controllers.finish_scope(controller_scope)?;
            Err(error.into())
        }
    }
}

pub(super) fn finish_controller_scope(
    In(scope): In<ControllerCommitScope>,
    mut controllers: ControllerLifecycleManagement,
) -> Result<(), CreatureAssemblyError> {
    Ok(controllers.finish_scope(scope)?)
}

pub(super) fn request_controller(
    In((scope, body, domain)): In<(ControllerCommitScope, ConceptBodyHandle, IntentDomain)>,
    mut controllers: ControllerLifecycleManagement,
) -> Result<ControllerLifecycleRequestId, CreatureAssemblyError> {
    Ok(controllers.register(scope, body, domain)?)
}

pub(super) fn request_controller_removal(
    In((scope, id)): In<(ControllerCommitScope, ControllerId)>,
    mut controllers: ControllerLifecycleManagement,
) -> Result<ControllerLifecycleRequestId, CreatureAssemblyError> {
    Ok(controllers.remove(scope, id)?)
}

pub(super) fn consume_controller_result(
    In((scope, id)): In<(ControllerCommitScope, ControllerLifecycleRequestId)>,
    mut controllers: ControllerLifecycleManagement,
) -> Option<ControllerLifecycleCompletion> {
    controllers.take_completion(scope, id)
}

fn commit_controller_request(
    world: &mut World,
    scope: ControllerCommitScope,
    id: ControllerLifecycleRequestId,
) -> Result<ControllerLifecycleCompletion, CreatureAssemblyError> {
    let committed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_scoped_controller_request(world, scope, id)
    }));
    let completion = world
        .run_system_once_with(consume_controller_result, (scope, id))
        .expect("bootstrap Controller consumer has its declared resource");
    match committed {
        Ok(result) => {
            result?;
            completion.ok_or(CreatureAssemblyError::InvalidBootstrap)
        }
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub(super) fn create_controller(
    world: &mut World,
    scope: ControllerCommitScope,
    body: ConceptBodyHandle,
    domain: IntentDomain,
) -> Result<ControllerId, CreatureAssemblyError> {
    let id = world
        .run_system_once_with(request_controller, (scope, body, domain))
        .expect("bootstrap Controller producer has its declared resource")?;
    match commit_controller_request(world, scope, id)? {
        ControllerLifecycleCompletion::Registered(result) => Ok(result?),
        _ => Err(CreatureAssemblyError::InvalidBootstrap),
    }
}

pub(super) fn remove_controllers(
    world: &mut World,
    scope: ControllerCommitScope,
    ids: impl IntoIterator<Item = ControllerId>,
) {
    for controller in ids {
        let id = world
            .run_system_once_with(request_controller_removal, (scope, controller))
            .expect("Controller compensation has its declared resource")
            .expect("Controller scope retains its compensation slot");
        match commit_controller_request(world, scope, id)
            .expect("Controller compensation uses its exact current request")
        {
            // Preserve best-effort removal when an observer already removed it.
            ControllerLifecycleCompletion::Removed(_) => {}
            _ => panic!("Controller compensation received a non-removal result"),
        }
    }
}

pub(super) fn finish_scope(
    In(scope): In<LifeCommitScope>,
    mut lifecycle: LifeScopedManagement,
) -> Result<(), CreatureAssemblyError> {
    Ok(lifecycle.finish_scope(scope)?)
}

pub(super) fn request_test_creature(
    In((scope, translation, environment)): In<(
        LifeCommitScope,
        bevy::prelude::Vec3,
        EnvironmentSample,
    )>,
    mut ids: ResMut<BodyInstanceIds>,
    mut lifecycle: LifeScopedManagement,
) -> Result<(ScopedLifeRequestId, TestCreatureRecipe), CreatureAssemblyError> {
    let prototype = TestLifePrototype::default();
    let (intent_id, execution_id, record_id) = (ids.intent(), ids.execution(), ids.record());
    let life = LifePrototype::empty()
        .with_state_policy::<GazeCommandState>(StatePolicy::initialized_only())
        .with_event_interface::<roundo_block_interaction::ItemUseCommandEvent>(EventPolicy::new(
            16,
            16 * std::mem::size_of::<roundo_block_interaction::ItemUseCommandEvent>(),
            1,
            1,
        )?)
        .with_state_policy::<MovementCommandState>(StatePolicy::initialized_only())
        .with_event_interface::<GazeCommandEvent>(EventPolicy::new(
            1,
            std::mem::size_of::<GazeCommandEvent>(),
            1,
            1,
        )?)
        .with_state_policy::<CreatureMotionExperienceState>(StatePolicy::initialized_only());
    let recipe = TestCreatureRecipe {
        intent: ConceptBodyPrototype::new(prototype.intent.materialize(intent_id))
            .with_state_writer::<GazeCommandState>()
            .with_state_reader::<GazeCommandState>()
            .with_event_writer::<roundo_block_interaction::ItemUseCommandEvent>()
            .with_state_writer::<MovementCommandState>()
            .with_event_writer::<GazeCommandEvent>(),
        execution: ConceptBodyPrototype::new(prototype.execution.materialize(
            execution_id,
            translation,
            environment,
        ))
        .with_state_reader::<GazeCommandState>()
        .with_event_reader::<roundo_block_interaction::ItemUseCommandEvent>()
        .with_state_reader::<MovementCommandState>()
        .with_event_reader::<GazeCommandEvent>()
        .with_state_writer::<CreatureMotionExperienceState>(),
        record: ConceptBodyPrototype::new(prototype.record.materialize(record_id))
            .with_state_reader::<CreatureMotionExperienceState>(),
    };
    Ok((lifecycle.create_life(scope, life)?, recipe))
}

pub(super) fn request_body<B: Bundle>(
    In((scope, prototype)): In<(LifeCommitScope, ConceptBodyPrototype<B>)>,
    mut lifecycle: LifeScopedManagement,
) -> Result<ScopedLifeRequestId, CreatureAssemblyError> {
    Ok(lifecycle.create_body(scope, prototype)?)
}

pub(super) fn request_slot<S: StateData>(
    In((scope, life, origin, initial)): In<(LifeCommitScope, LifeHandle, ConceptBodyHandle, S)>,
    mut lifecycle: LifeScopedManagement,
) -> Result<ScopedLifeRequestId, CreatureAssemblyError> {
    Ok(lifecycle.create_state_slot(scope, life, origin, Some(initial))?)
}

pub(super) fn request_body_removal(
    In((scope, body)): In<(LifeCommitScope, ConceptBodyHandle)>,
    mut lifecycle: LifeScopedManagement,
) -> Result<ScopedLifeRequestId, CreatureAssemblyError> {
    Ok(lifecycle.remove_body(scope, body)?)
}

pub(super) fn request_life_removal(
    In((scope, life)): In<(LifeCommitScope, LifeHandle)>,
    mut lifecycle: LifeScopedManagement,
) -> Result<ScopedLifeRequestId, CreatureAssemblyError> {
    Ok(lifecycle.remove_life(scope, life)?)
}

pub(super) fn consume_result(
    In((scope, id)): In<(LifeCommitScope, ScopedLifeRequestId)>,
    mut lifecycle: LifeScopedManagement,
) -> Option<ScopedLifeCompletion> {
    lifecycle.take_completion(scope, id)
}

/// Retained synchronous coordination only: select the exact closed structural
/// request, then consume its actual result through a native system. Even when
/// the kernel rethrows a panic, consume Panicked to free the scope's reserved
/// slot before outer compensation. No callback is handed a World.
fn commit_request(
    world: &mut World,
    scope: LifeCommitScope,
    id: ScopedLifeRequestId,
) -> Result<ScopedLifeCompletion, CreatureAssemblyError> {
    let committed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_scoped_life_request(world, scope, id)
    }));
    let completion = world
        .run_system_once_with(consume_result, (scope, id))
        .expect("bootstrap result consumer has its declared lifecycle resource");
    match committed {
        Ok(result) => {
            result?;
            completion.ok_or(CreatureAssemblyError::InvalidBootstrap)
        }
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub(super) fn created_body<B: Bundle>(
    world: &mut World,
    scope: LifeCommitScope,
    prototype: ConceptBodyPrototype<B>,
) -> Result<ConceptBodyHandle, CreatureAssemblyError> {
    let id = world
        .run_system_once_with(request_body::<B>, (scope, prototype))
        .expect("bootstrap Body request has its declared lifecycle resource")?;
    match commit_request(world, scope, id)? {
        ScopedLifeCompletion::Factory(FactoryCompletion::BodyCreated(body)) => Ok(body),
        ScopedLifeCompletion::Factory(FactoryCompletion::Rejected(error)) => Err(error.into()),
        _ => Err(CreatureAssemblyError::InvalidBootstrap),
    }
}

pub(super) fn created_slot<S: StateData>(
    world: &mut World,
    scope: LifeCommitScope,
    life: LifeHandle,
    origin: ConceptBodyHandle,
    initial: S,
) -> Result<(), CreatureAssemblyError> {
    let id = world
        .run_system_once_with(request_slot::<S>, (scope, life, origin, initial))
        .expect("bootstrap Slot request has its declared lifecycle resource")?;
    match commit_request(world, scope, id)? {
        ScopedLifeCompletion::State(result) => Ok(result?),
        _ => Err(CreatureAssemblyError::InvalidBootstrap),
    }
}

pub(super) fn remove_instances(
    world: &mut World,
    scope: LifeCommitScope,
    life: LifeHandle,
    bodies: impl IntoIterator<Item = ConceptBodyHandle>,
) {
    for body in bodies {
        let id = world
            .run_system_once_with(request_body_removal, (scope, body))
            .expect("bootstrap compensation has its declared lifecycle resource")
            .expect("bootstrap scope retains its compensation slot");
        finish_removal(world, scope, id);
    }
    let id = world
        .run_system_once_with(request_life_removal, (scope, life))
        .expect("bootstrap compensation has its declared lifecycle resource")
        .expect("bootstrap scope retains its compensation slot");
    finish_removal(world, scope, id);
}

fn finish_removal(world: &mut World, scope: LifeCommitScope, id: ScopedLifeRequestId) {
    match commit_request(world, scope, id)
        .expect("compensation uses its own current scoped request")
    {
        // Preserve the previous best-effort cleanup of already removed Handles
        // and typed Factory rejection; protocol/result mismatches are not ignored.
        ScopedLifeCompletion::Factory(
            FactoryCompletion::Removed | FactoryCompletion::Rejected(_),
        ) => {}
        _ => panic!("bootstrap compensation received a non-removal result"),
    }
}

pub(super) fn create_test_life(
    world: &mut World,
    scope: LifeCommitScope,
    translation: bevy::prelude::Vec3,
    environment: EnvironmentSample,
) -> Result<(LifeHandle, TestCreatureRecipe), CreatureAssemblyError> {
    let (id, recipe) = world
        .run_system_once_with(request_test_creature, (scope, translation, environment))
        .expect("bootstrap request has its declared recipe resources")?;
    match commit_request(world, scope, id)? {
        ScopedLifeCompletion::Factory(FactoryCompletion::LifeCreated(life)) => Ok((life, recipe)),
        ScopedLifeCompletion::Factory(FactoryCompletion::Rejected(error)) => Err(error.into()),
        _ => Err(CreatureAssemblyError::InvalidBootstrap),
    }
}
