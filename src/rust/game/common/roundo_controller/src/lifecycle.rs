//! Bounded, host-selected Controller lifecycle and native structural rules.
//! Requests carry closed operations, never a caller callback or World capability.

use super::*;
use bevy::ecs::{
    system::{SystemParam, SystemState},
    world::WorldId,
};
use bevy::prelude::{Commands, FromWorld, With};
use roundo_life::LifeInstances;

/// App-affine scope for an already serialized host, not Controller authority.
/// Each App permits one live scope with one pending/in-flight/unclaimed slot.
/// The host must consume all results and finish the scope, including on unwind.
///
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ControllerCommitScope {
    world: WorldId,
    sequence: u64,
}

/// Correlates one closed operation in a live scope; not a Controller identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ControllerLifecycleRequestId {
    scope: ControllerCommitScope,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerLifecycleError {
    ScopeBusy,
    InvalidScope,
    OutstandingRequest,
    UnknownRequest,
    IdExhausted,
}

/// Available only after targeted structural application and validation complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerLifecycleCompletion {
    Registered(Result<ControllerId, RegisterControllerError>),
    Removed(Result<(), ControllerControlError>),
    /// Recorded before rethrowing the original panic. Creation compensates its
    /// own Controller/Access; removal can already be effective. Never auto-retry.
    Panicked,
}

#[derive(Clone, Copy)]
enum Operation {
    Register(ConceptBodyHandle, IntentDomain),
    Remove(ControllerId),
}
struct ActiveScope {
    scope: ControllerCommitScope,
    next_sequence: u64,
    pending: Option<(ControllerLifecycleRequestId, Operation)>,
    completed: Option<(ControllerLifecycleRequestId, ControllerLifecycleCompletion)>,
    in_flight: bool,
}
impl ActiveScope {
    fn occupied(&self) -> bool {
        self.pending.is_some() || self.completed.is_some() || self.in_flight
    }
}
#[derive(Resource)]
pub(crate) struct ControllerLifecycleRequests {
    world: WorldId,
    next_scope: u64,
    active: Option<ActiveScope>,
}
impl FromWorld for ControllerLifecycleRequests {
    fn from_world(world: &mut World) -> Self {
        Self {
            world: world.id(),
            next_scope: 1,
            active: None,
        }
    }
}

/// Native lifecycle-only Interface for the existing serial bootstrap host.
/// Enqueue is not success; it changes neither registry, Access nor entities.
/// Commit exactly the returned ID, then take its real result before proceeding.
/// The single slot stays reserved across idle stages, so observers cannot steal
/// compensation capacity or finish an in-flight scope. This lane is independent
/// of Life's bounded queue and never runs from, drains or advances FixedUpdate.
/// No Life registration/data/gate capability, World, callback or Commands is exposed.
#[derive(SystemParam)]
pub struct ControllerLifecycleManagement<'w> {
    requests: ResMut<'w, ControllerLifecycleRequests>,
}
impl ControllerLifecycleManagement<'_> {
    pub fn begin_scope(&mut self) -> Result<ControllerCommitScope, ControllerLifecycleError> {
        if self.requests.active.is_some() {
            return Err(ControllerLifecycleError::ScopeBusy);
        }
        let sequence = self.requests.next_scope;
        self.requests.next_scope = sequence
            .checked_add(1)
            .ok_or(ControllerLifecycleError::IdExhausted)?;
        let scope = ControllerCommitScope {
            world: self.requests.world,
            sequence,
        };
        self.requests.active = Some(ActiveScope {
            scope,
            next_sequence: 1,
            pending: None,
            completed: None,
            in_flight: false,
        });
        Ok(scope)
    }
    fn active(
        &mut self,
        scope: ControllerCommitScope,
    ) -> Result<&mut ActiveScope, ControllerLifecycleError> {
        self.requests
            .active
            .as_mut()
            .filter(|active| active.scope == scope)
            .ok_or(ControllerLifecycleError::InvalidScope)
    }
    fn submit(
        &mut self,
        scope: ControllerCommitScope,
        operation: Operation,
    ) -> Result<ControllerLifecycleRequestId, ControllerLifecycleError> {
        let active = self.active(scope)?;
        if active.occupied() {
            return Err(ControllerLifecycleError::OutstandingRequest);
        }
        let sequence = active.next_sequence;
        active.next_sequence = sequence
            .checked_add(1)
            .ok_or(ControllerLifecycleError::IdExhausted)?;
        let id = ControllerLifecycleRequestId { scope, sequence };
        active.pending = Some((id, operation));
        Ok(id)
    }
    pub fn register(
        &mut self,
        scope: ControllerCommitScope,
        body: ConceptBodyHandle,
        domain: IntentDomain,
    ) -> Result<ControllerLifecycleRequestId, ControllerLifecycleError> {
        self.submit(scope, Operation::Register(body, domain))
    }
    pub fn remove(
        &mut self,
        scope: ControllerCommitScope,
        id: ControllerId,
    ) -> Result<ControllerLifecycleRequestId, ControllerLifecycleError> {
        self.submit(scope, Operation::Remove(id))
    }
    pub fn take_completion(
        &mut self,
        scope: ControllerCommitScope,
        id: ControllerLifecycleRequestId,
    ) -> Option<ControllerLifecycleCompletion> {
        let active = self.active(scope).ok()?;
        if active.completed.as_ref()?.0 != id {
            return None;
        }
        active.completed.take().map(|(_, result)| result)
    }
    pub fn cancel_request(
        &mut self,
        scope: ControllerCommitScope,
        id: ControllerLifecycleRequestId,
    ) -> Result<(), ControllerLifecycleError> {
        let active = self.active(scope)?;
        if active.pending.as_ref().map(|(pending, _)| *pending) != Some(id) {
            return Err(ControllerLifecycleError::UnknownRequest);
        }
        active.pending = None;
        Ok(())
    }
    pub fn finish_scope(
        &mut self,
        scope: ControllerCommitScope,
    ) -> Result<(), ControllerLifecycleError> {
        if self.active(scope)?.occupied() {
            return Err(ControllerLifecycleError::OutstandingRequest);
        }
        self.requests.active = None;
        Ok(())
    }
}

/// Selects only the current exact scoped operation. All role/generation/domain
/// checks and registry/Access mutation use native parameters below; this bridge
/// only coordinates targeted deferred application, actual result publication and
/// unwind. It never flushes Life's request FIFO, advances time or calls user code
/// with World. Foreign/stale/unknown IDs reject before taking any operation.
pub fn commit_scoped_controller_request(
    world: &mut World,
    scope: ControllerCommitScope,
    id: ControllerLifecycleRequestId,
) -> Result<(), ControllerLifecycleError> {
    let operation = {
        let mut requests = world
            .get_resource_mut::<ControllerLifecycleRequests>()
            .ok_or(ControllerLifecycleError::InvalidScope)?;
        let active = requests
            .active
            .as_mut()
            .filter(|active| active.scope == scope)
            .ok_or(ControllerLifecycleError::InvalidScope)?;
        if active.pending.as_ref().map(|(pending, _)| *pending) != Some(id) {
            return Err(ControllerLifecycleError::UnknownRequest);
        }
        let (_, operation) = active.pending.take().expect("checked pending request");
        active.in_flight = true;
        operation
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match operation {
        Operation::Register(body, domain) => ControllerLifecycleCompletion::Registered(
            register_target(world, ControllerTarget::Factory(body), domain),
        ),
        Operation::Remove(controller) => {
            ControllerLifecycleCompletion::Removed(remove(world, controller))
        }
    }));
    let mut requests = world.resource_mut::<ControllerLifecycleRequests>();
    let active = requests
        .active
        .as_mut()
        .expect("in-flight scope cannot finish");
    active.in_flight = false;
    active.completed = Some((
        id,
        result
            .as_ref()
            .copied()
            .unwrap_or(ControllerLifecycleCompletion::Panicked),
    ));
    drop(requests);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    Ok(())
}

// Observers select targets; this narrow parameter owns the retirement protocol
// without requiring Life or registration queries.
#[derive(SystemParam)]
pub(super) struct ControllerRetirement<'w, 's> {
    commands: Commands<'w, 's>,
    registry: ResMut<'w, ControllerRegistry>,
    players: Query<'w, 's, &'static mut Player>,
}
impl ControllerRetirement<'_, '_> {
    fn clear_access(&mut self, id: ControllerId) {
        for mut player in &mut self.players {
            player.set_controller_access(id, ControllerAccessLevel::None);
        }
    }
    fn clear_access_and_despawn(&mut self, id: ControllerId, entity: Entity) {
        // Revoke before deferred removal hooks, including when they panic.
        self.clear_access(id);
        self.commands.entity(entity).try_despawn();
    }
    pub(super) fn retire_selected(&mut self, id: ControllerId, entity: Entity) {
        if self.registry.unregister_id(id).is_none() {
            return;
        }
        self.clear_access_and_despawn(id, entity);
    }
    fn cleanup_exact(&mut self, id: ControllerId, entity: Entity) {
        if self.registry.controller_entity(id) == Some(entity) {
            self.registry.unregister_id(id);
        }
        self.clear_access_and_despawn(id, entity);
    }
    pub(super) fn reconcile_removed(&mut self, entity: Entity) {
        let Some(id) = self.registry.unregister_entity(entity) else {
            return;
        };
        self.clear_access(id);
    }
}

// This parameter is deliberately private: callers request closed operations;
// only this owning Implementation receives targeted structural Commands.
#[derive(SystemParam)]
struct ControllerStructure<'w, 's> {
    retirement: ControllerRetirement<'w, 's>,
    instances: LifeInstances<'w, 's>,
    intents: Query<'w, 's, (), With<IntentBodyId>>,
    controllers: Query<'w, 's, &'static Controller>,
}
impl ControllerStructure<'_, '_> {
    fn target_entity(&self, target: ControllerTarget) -> Option<Entity> {
        match target {
            ControllerTarget::Factory(body) => self.instances.resolve_body(body).ok(),
            ControllerTarget::Compatibility(entity) => Some(entity),
        }
    }
    fn register(
        &mut self,
        target: ControllerTarget,
        domain: IntentDomain,
    ) -> Result<(ControllerId, Entity), RegisterControllerError> {
        let intent = self
            .target_entity(target)
            .filter(|&entity| self.intents.contains(entity))
            .ok_or(RegisterControllerError::IntentBodyNotFound)?;
        for &entity in self.retirement.registry.entities_by_id.values() {
            let Ok(existing) = self.controllers.get(entity) else {
                continue;
            };
            if self.target_entity(existing.target) == Some(intent)
                && existing.domain.overlaps(domain)
            {
                return Err(RegisterControllerError::OverlappingIntentDomain {
                    existing_controller: existing.id,
                });
            }
        }
        let id = ControllerId(self.retirement.registry.next_controller_id);
        self.retirement.registry.next_controller_id = self
            .retirement
            .registry
            .next_controller_id
            .checked_add(1)
            .ok_or(RegisterControllerError::IdExhausted)?;
        // Retain the reserved entity before deferred component hooks run, so even
        // an on-add panic before a Controller exists has an exact cleanup target.
        let entity = self
            .retirement
            .commands
            .spawn(Controller {
                id,
                target,
                domain,
                current_owner: None,
            })
            .id();
        self.retirement.registry.entities_by_id.insert(id, entity);
        Ok((id, entity))
    }
    fn registration_live(
        &self,
        id: ControllerId,
        entity: Entity,
        target: ControllerTarget,
        domain: IntentDomain,
    ) -> bool {
        self.retirement.registry.controller_entity(id) == Some(entity)
            && self.controllers.get(entity).is_ok_and(|controller| {
                controller.id == id && controller.target == target && controller.domain == domain
            })
            && self
                .target_entity(target)
                .is_some_and(|intent| self.intents.contains(intent))
    }
    fn remove(&mut self, id: ControllerId) -> Result<(), ControllerControlError> {
        let entity = self
            .retirement
            .registry
            .controller_entity(id)
            .filter(|&entity| {
                self.controllers
                    .get(entity)
                    .is_ok_and(|controller| controller.id == id)
            })
            .ok_or(ControllerControlError::ControllerNotFound)?;
        self.retirement.cleanup_exact(id, entity);
        Ok(())
    }
}

// Synchronous compatibility/selected-commit bridges. They share native rules;
// they are not permission for ordinary adapters to inspect or mutate World.
pub(crate) fn register_target(
    world: &mut World,
    target: ControllerTarget,
    domain: IntentDomain,
) -> Result<ControllerId, RegisterControllerError> {
    world.init_resource::<ControllerRegistry>();
    let mut state = SystemState::<ControllerStructure>::new(world);
    let (id, entity) = state
        .get_mut(world)
        .expect("Controller registry initialized")
        .register(target, domain)?;
    let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.apply(world)));
    let live = applied.is_ok()
        && state
            .get_mut(world)
            .expect("Controller registry initialized")
            .registration_live(id, entity, target, domain);
    if !live {
        // Use a new command buffer after a deferred hook panic. Cleanup is exact,
        // and cannot release an unrelated Controller or replacement Body.
        let mut cleanup = SystemState::<ControllerRetirement>::new(world);
        cleanup
            .get_mut(world)
            .expect("Controller registry initialized")
            .cleanup_exact(id, entity);
        cleanup.apply(world);
    }
    if let Err(payload) = applied {
        std::panic::resume_unwind(payload);
    }
    if live {
        Ok(id)
    } else {
        Err(RegisterControllerError::IntentBodyNotFound)
    }
}
pub(crate) fn remove(world: &mut World, id: ControllerId) -> Result<(), ControllerControlError> {
    world.init_resource::<ControllerRegistry>();
    let mut state = SystemState::<ControllerStructure>::new(world);
    state
        .get_mut(world)
        .expect("Controller registry initialized")
        .remove(id)?;
    state.apply(world);
    Ok(())
}
