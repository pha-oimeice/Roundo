//! Authoritative Controller registration, access checks, and exclusive control.
//!
//! Controllers are ECS entities external to their target Intent Bodies. This
//! crate owns the controller-ID index and current-control truth; Players retain
//! the separate Controller Access truth in `roundo_player`.

pub mod authority_gate;
mod lifecycle;
pub use lifecycle::{
    ControllerCommitScope, ControllerLifecycleCompletion, ControllerLifecycleError,
    ControllerLifecycleManagement, ControllerLifecycleRequestId, commit_scoped_controller_request,
};

use bevy::prelude::{App, Component, Entity, On, Plugin, Query, Remove, ResMut, Resource, World};
use roundo_contracts::{ControllerAccessLevel, ControllerId, PlayerId};
use roundo_creature::IntentBodyId;
use roundo_life::{
    ConceptBodyFactory, ConceptBodyHandle, ConceptBodyIdentity, FactoryInvalidation,
};
use roundo_player::{Player, PlayerRegistry};
use std::collections::{HashMap, HashSet};

/// Server-local AI identity minted by the trusted Controller registration boundary.
/// Not a wire identity or an ECS Entity; IDs are never reused in this runtime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AiControllerId(u64);

/// The active operator, distinct from Intent Body lifecycle ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerOwner {
    Player(PlayerId),
    Ai(AiControllerId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiRegistrationError {
    IdExhausted,
}

/// Trusted server-local registration; never invoke this from a wire request.
pub fn register_ai(world: &mut World) -> Result<AiControllerId, AiRegistrationError> {
    world.init_resource::<ControllerRegistry>();
    let mut registry = world.resource_mut::<ControllerRegistry>();
    let id = registry.next_ai_id;
    registry.next_ai_id = id.checked_add(1).ok_or(AiRegistrationError::IdExhausted)?;
    let id = AiControllerId(id);
    registry.active_ai.insert(id);
    Ok(id)
}

/// Ends an AI producer's lifetime and releases all of its current controls.
pub fn remove_ai(world: &mut World, ai: AiControllerId) -> Result<usize, ControllerControlError> {
    let entities = {
        let mut registry = world
            .get_resource_mut::<ControllerRegistry>()
            .ok_or(ControllerControlError::AiNotFound)?;
        if !registry.active_ai.remove(&ai) {
            return Err(ControllerControlError::AiNotFound);
        }
        registry
            .entities_by_id
            .values()
            .copied()
            .collect::<Vec<_>>()
    };
    let mut released = 0;
    for entity in entities {
        let target_is_live = world
            .get::<Controller>(entity)
            .and_then(|controller| {
                (controller.current_owner == Some(ControllerOwner::Ai(ai)))
                    .then_some(())
                    .and_then(|_| controller.resolve_intent_body(world))
            })
            .is_some();
        if target_is_live {
            world.get_mut::<Controller>(entity).unwrap().current_owner = None;
            released += 1;
        }
    }
    Ok(released)
}

/// One closed source-defined kind of Intent a Controller may submit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IntentKind {
    Movement,
    Gaze,
}

/// Immutable non-empty set of Intent kinds emitted by one Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntentDomain(u8);

impl IntentDomain {
    const MOVEMENT: u8 = 1;
    const GAZE: u8 = 2;
    /// The singleton domain for movement intents.
    pub const fn movement() -> Self {
        Self(Self::MOVEMENT)
    }

    /// The singleton domain for gaze intents.
    pub const fn gaze() -> Self {
        Self(Self::GAZE)
    }

    /// Creates a non-empty domain from source-defined intent kinds.
    pub fn new(kinds: impl IntoIterator<Item = IntentKind>) -> Result<Self, IntentDomainError> {
        let mut bits = 0;
        for kind in kinds {
            bits |= match kind {
                IntentKind::Movement => Self::MOVEMENT,
                IntentKind::Gaze => Self::GAZE,
            };
        }
        if bits == 0 {
            return Err(IntentDomainError::Empty);
        }
        Ok(Self(bits))
    }

    pub const fn contains(self, kind: IntentKind) -> bool {
        let bit = match kind {
            IntentKind::Movement => Self::MOVEMENT,
            IntentKind::Gaze => Self::GAZE,
        };
        self.0 & bit != 0
    }

    pub const fn overlaps(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// Invalid Intent Domain construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentDomainError {
    Empty,
}

/// Input submitted through an acquired Controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControllerInput {
    Movement { local_axis: [f32; 3] },
    Gaze { yaw_delta: f32, pitch_delta: f32 },
}

impl ControllerInput {
    pub const fn kind(self) -> IntentKind {
        match self {
            Self::Movement { .. } => IntentKind::Movement,
            Self::Gaze { .. } => IntentKind::Gaze,
        }
    }
}

/// Authoritative runtime Controller state.
#[derive(Component, Debug, Eq, PartialEq)]
pub struct Controller {
    id: ControllerId,
    target: ControllerTarget,
    domain: IntentDomain,
    current_owner: Option<ControllerOwner>,
}

/// Factory Controllers retain only their generational handle. Entity targets are
/// an explicit compatibility path, not a Factory provenance guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControllerTarget {
    Factory(ConceptBodyHandle),
    Compatibility(Entity),
}

fn resolve_target(world: &World, target: ControllerTarget) -> Option<Entity> {
    match target {
        ControllerTarget::Factory(handle) => ConceptBodyFactory::resolve_entity(world, handle).ok(),
        ControllerTarget::Compatibility(entity) => Some(entity),
    }
}

impl Controller {
    pub const fn id(&self) -> ControllerId {
        self.id
    }

    /// Stable Concept Body target for a factory-composed Controller.
    pub const fn intent_body_handle(&self) -> Option<ConceptBodyHandle> {
        match self.target {
            ControllerTarget::Factory(handle) => Some(handle),
            ControllerTarget::Compatibility(_) => None,
        }
    }

    fn resolve_intent_body(&self, world: &World) -> Option<Entity> {
        resolve_target(world, self.target)
    }

    pub const fn domain(&self) -> IntentDomain {
        self.domain
    }

    pub const fn current_owner(&self) -> Option<ControllerOwner> {
        self.current_owner
    }

    /// Player-only compatibility projection; AI occupation appears as None.
    pub const fn current_player(&self) -> Option<PlayerId> {
        match self.current_owner {
            Some(ControllerOwner::Player(player)) => Some(player),
            _ => None,
        }
    }
}

/// In-memory index for Controller runtime identities.
#[derive(Resource, Debug, Default)]
pub struct ControllerRegistry {
    next_controller_id: u64,
    entities_by_id: HashMap<ControllerId, Entity>,
    reclaimed_controller_ids: Vec<ControllerId>,
    next_ai_id: u64,
    active_ai: HashSet<AiControllerId>,
}

impl ControllerRegistry {
    /// Trusted local AI registration status, not a wire-authentication claim.
    pub fn is_ai_registered(&self, ai: AiControllerId) -> bool {
        self.active_ai.contains(&ai)
    }

    /// Returns the ECS entity currently registered for an ID.
    pub fn controller_entity(&self, controller_id: ControllerId) -> Option<Entity> {
        self.entities_by_id.get(&controller_id).copied()
    }

    /// Drains IDs reclaimed since the previous call.
    ///
    /// Composition layers use this to discard Controller-keyed auxiliary state;
    /// registry lookup and authorization already reject these IDs immediately.
    pub fn take_reclaimed_controller_ids(&mut self) -> Vec<ControllerId> {
        std::mem::take(&mut self.reclaimed_controller_ids)
    }

    fn unregister_id(&mut self, controller_id: ControllerId) -> Option<Entity> {
        let entity = self.entities_by_id.remove(&controller_id)?;
        self.reclaimed_controller_ids.push(controller_id);
        Some(entity)
    }

    fn unregister_entity(&mut self, entity: Entity) -> Option<ControllerId> {
        let controller_id = self
            .entities_by_id
            .iter()
            .find_map(|(&id, &candidate)| (candidate == entity).then_some(id))?;
        self.unregister_id(controller_id);
        Some(controller_id)
    }
}

/// Installs event-driven Controller lifecycle enforcement.
///
/// Cleanup runs only when a Player, Intent Body, or Controller is removed; it
/// never performs a per-frame validity scan. A Controller with a live Intent
/// Body remains registered even when no Player has Access to it.
#[derive(Default)]
pub struct ControllerLifecyclePlugin;

impl Plugin for ControllerLifecyclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ControllerRegistry>()
            .init_resource::<PlayerRegistry>()
            .init_resource::<lifecycle::ControllerLifecycleRequests>()
            .add_observer(reclaim_removed_player)
            .add_observer(reclaim_removed_intent_body)
            .add_observer(reclaim_invalidated_factory_body)
            .add_observer(reconcile_removed_controller);
    }
}

fn reclaim_removed_player(
    removal: On<Remove<Player>>,
    mut players: ResMut<PlayerRegistry>,
    mut controllers: Query<&mut Controller>,
) {
    let Some(player_id) = players.remove_player_entity(removal.entity) else {
        return;
    };
    for mut controller in &mut controllers {
        if controller.current_owner == Some(ControllerOwner::Player(player_id)) {
            controller.current_owner = None;
        }
    }
}

fn reclaim_removed_intent_body(
    removal: On<Remove<IntentBodyId>>,
    identities: Query<&ConceptBodyIdentity>,
    controllers: Query<(Entity, &Controller)>,
    mut retirement: lifecycle::ControllerRetirement,
) {
    let factory_handle = identities
        .get(removal.entity)
        .ok()
        .map(ConceptBodyIdentity::handle);
    let reclaimed = controllers
        .iter()
        .filter_map(|(entity, controller)| {
            let matches = match controller.target {
                ControllerTarget::Compatibility(intent) => intent == removal.entity,
                ControllerTarget::Factory(handle) => factory_handle == Some(handle),
            };
            matches.then_some((controller.id, entity))
        })
        .collect::<Vec<_>>();
    for (controller_id, entity) in reclaimed {
        retirement.retire_selected(controller_id, entity);
    }
}

/// Factory loss revokes the exact Handle even if a caller already removed its
/// Intent role component. Registry and Access invalidation are immediate;
/// targeted despawn is flushed by the Factory commit before it returns.
fn reclaim_invalidated_factory_body(
    invalidation: On<FactoryInvalidation>,
    controllers: Query<(Entity, &Controller)>,
    mut retirement: lifecycle::ControllerRetirement,
) {
    let FactoryInvalidation::ConceptBody(body) = *invalidation else {
        return;
    };
    for (entity, controller) in &controllers {
        if controller.intent_body_handle() != Some(body) {
            continue;
        }
        retirement.retire_selected(controller.id(), entity);
    }
}

fn reconcile_removed_controller(
    removal: On<Remove<Controller>>,
    mut retirement: lifecycle::ControllerRetirement,
) {
    retirement.reconcile_removed(removal.entity);
}

/// Failure to register a Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterControllerError {
    IntentBodyNotFound,
    IdExhausted,
    OverlappingIntentDomain { existing_controller: ControllerId },
}

/// Registers one Controller and atomically checks the target and domain.
///
/// The target must carry `IntentBodyId`. The target is never modified: this
/// external relation preserves the Intent Body's lack of Controller knowledge.
/// Explicit compatibility constructor for legacy Entity-only callers.
pub fn register_controller(
    world: &mut World,
    intent_body: Entity,
    domain: IntentDomain,
) -> Result<ControllerId, RegisterControllerError> {
    lifecycle::register_target(world, ControllerTarget::Compatibility(intent_body), domain)
}

/// Registers a factory-composed Controller whose persistent routing truth is
/// exclusively the generational Concept Body handle.
pub fn register_factory_controller(
    world: &mut World,
    intent_body: ConceptBodyHandle,
    domain: IntentDomain,
) -> Result<ControllerId, RegisterControllerError> {
    lifecycle::register_target(world, ControllerTarget::Factory(intent_body), domain)
}

/// Removes a Controller through its owning registry lifecycle.
///
/// Removes all factory Controllers bound to this exact handle. Composition calls
/// this after factory invalidation, so a stale generation can never route to a
/// replacement Body.
pub fn remove_factory_controllers_for_body(world: &mut World, body: ConceptBodyHandle) {
    let ids = world
        .get_resource::<ControllerRegistry>()
        .map(|registry| {
            registry
                .entities_by_id
                .values()
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entity| {
            world.get::<Controller>(entity).and_then(|controller| {
                (controller.intent_body_handle() == Some(body)).then_some(controller.id)
            })
        })
        .collect::<Vec<_>>();
    for id in ids {
        let _ = remove_controller(world, id);
    }
}

/// Synchronous bridge to the native owning lifecycle rules. Registry and all
/// Player Access are revoked before targeted despawn, including on hook panic.
/// Install [`ControllerLifecyclePlugin`] for automatic target/Player cleanup.
pub fn remove_controller(
    world: &mut World,
    controller_id: ControllerId,
) -> Result<(), ControllerControlError> {
    lifecycle::remove(world, controller_id)
}

/// Outcome of a successful acquire request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquireControlOutcome {
    Acquired,
    AlreadyControlled,
}

/// Returns routing facts and a Player-only compatibility projection of control.
/// AI occupation is not represented in the second field; use `controller_owner`.
pub fn controller_state(
    world: &World,
    controller_id: ControllerId,
) -> Option<(IntentDomain, Option<PlayerId>)> {
    let entity = controller_entity(world, controller_id).ok()?;
    let controller = world.get::<Controller>(entity)?;
    Some((controller.domain, controller.current_player()))
}

/// Returns the unique Intent Body targeted by a live Controller.
///
/// The caller may route an already authorized input to this entity, but must not
/// attach a reverse Controller reference to the Intent Body.
pub fn controller_intent_body(world: &World, controller_id: ControllerId) -> Option<Entity> {
    let entity = controller_entity(world, controller_id).ok()?;
    world.get::<Controller>(entity)?.resolve_intent_body(world)
}

pub fn controller_intent_body_handle(
    world: &World,
    controller_id: ControllerId,
) -> Option<ConceptBodyHandle> {
    let entity = controller_entity(world, controller_id).ok()?;
    world.get::<Controller>(entity)?.intent_body_handle()
}

/// Failure to acquire, release, or submit through a Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerControlError {
    ControllerNotFound,
    PlayerNotFound,
    AiNotFound,
    AccessDenied { level: ControllerAccessLevel },
    ControlledByOther { controller: ControllerId },
    NotCurrentController { controller: ControllerId },
    InputOutsideDomain { kind: IntentKind },
}

/// Acquires exclusive non-preemptive control for a Read-Write Player.
pub fn acquire_control(
    world: &mut World,
    player_id: PlayerId,
    controller_id: ControllerId,
) -> Result<AcquireControlOutcome, ControllerControlError> {
    acquire_owner(world, ControllerOwner::Player(player_id), controller_id)
}

/// Retired immediate authority API. Neither wire callers nor local producers may
/// establish a second owner outside the serialized gate.
pub fn acquire_owner(
    world: &mut World,
    _owner: ControllerOwner,
    controller_id: ControllerId,
) -> Result<AcquireControlOutcome, ControllerControlError> {
    controller_entity(world, controller_id)?;
    Err(ControllerControlError::AccessDenied {
        level: ControllerAccessLevel::None,
    })
}

/// Returns the current local owner without exposing it on the wire.
pub fn controller_owner(world: &World, controller_id: ControllerId) -> Option<ControllerOwner> {
    let entity = controller_entity(world, controller_id).ok()?;
    world.get::<Controller>(entity)?.current_owner
}

/// Releases only the Player's own control.
pub fn release_control(
    world: &mut World,
    player_id: PlayerId,
    controller_id: ControllerId,
) -> Result<(), ControllerControlError> {
    release_owner(world, ControllerOwner::Player(player_id), controller_id)
}

/// Releases only this live owner's control.
pub fn release_owner(
    world: &mut World,
    owner: ControllerOwner,
    controller_id: ControllerId,
) -> Result<(), ControllerControlError> {
    let entity = controller_entity(world, controller_id)?;
    // Preserve the existing Player release semantics: no Access check.
    if let ControllerOwner::Ai(ai) = owner {
        require_live_ai(world, ai)?;
    }
    let mut controller = world
        .get_mut::<Controller>(entity)
        .expect("registered Controller exists");
    if controller.current_owner != Some(owner) {
        return Err(ControllerControlError::NotCurrentController {
            controller: controller_id,
        });
    }
    controller.current_owner = None;
    drop(controller);
    // An owner left by a prior runtime may be revoked, but the retired direct
    // release API must never report a successful handoff.
    Err(ControllerControlError::AccessDenied {
        level: ControllerAccessLevel::None,
    })
}

/// Releases every Controller controlled by a Player after its last disconnect.
pub fn release_all_control(world: &mut World, player_id: PlayerId) -> usize {
    let entities = world
        .get_resource::<ControllerRegistry>()
        .map(|registry| {
            registry
                .entities_by_id
                .values()
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut released = 0;
    for entity in entities {
        let Some(mut controller) = world.get_mut::<Controller>(entity) else {
            continue;
        };
        if controller.current_owner == Some(ControllerOwner::Player(player_id)) {
            controller.current_owner = None;
            drop(controller);
            released += 1;
        }
    }
    released
}

/// Immediately validates control and domain before returning a typed input.
pub fn submit_input(
    world: &World,
    player_id: PlayerId,
    controller_id: ControllerId,
    input: ControllerInput,
) -> Result<ControllerInput, ControllerControlError> {
    submit_owner_input(
        world,
        ControllerOwner::Player(player_id),
        controller_id,
        input,
    )
}

/// Retired immediate Submit API. Input requires the gate's active binding.
pub fn submit_owner_input(
    world: &World,
    _owner: ControllerOwner,
    controller_id: ControllerId,
    _input: ControllerInput,
) -> Result<ControllerInput, ControllerControlError> {
    controller_entity(world, controller_id)?;
    Err(ControllerControlError::AccessDenied {
        level: ControllerAccessLevel::None,
    })
}

fn require_live_ai(world: &World, ai: AiControllerId) -> Result<(), ControllerControlError> {
    if world
        .get_resource::<ControllerRegistry>()
        .is_some_and(|registry| registry.active_ai.contains(&ai))
    {
        Ok(())
    } else {
        Err(ControllerControlError::AiNotFound)
    }
}

fn controller_entity(
    world: &World,
    controller_id: ControllerId,
) -> Result<Entity, ControllerControlError> {
    let Some(registry) = world.get_resource::<ControllerRegistry>() else {
        return Err(ControllerControlError::ControllerNotFound);
    };
    let Some(entity) = registry.controller_entity(controller_id) else {
        return Err(ControllerControlError::ControllerNotFound);
    };
    if world.get::<Controller>(entity).is_none() {
        return Err(ControllerControlError::ControllerNotFound);
    }
    Ok(entity)
}
