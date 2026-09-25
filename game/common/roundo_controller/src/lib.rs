//! Authoritative Controller registration, access checks, and exclusive control.
//!
//! Controllers are ECS entities external to their target Intent Bodies. This
//! crate owns the controller-ID index and current-control truth; Players retain
//! the separate Controller Access truth in `roundo_player`.

use bevy::prelude::{Component, Entity, Resource, World};
use roundo_contracts::{ControllerAccessLevel, ControllerId, PlayerId};
use roundo_creature::IntentBodyId;
use roundo_player::{Player, PlayerRegistry};
use std::collections::HashMap;

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
    intent_body: Entity,
    domain: IntentDomain,
    current_player: Option<PlayerId>,
}

impl Controller {
    pub const fn id(&self) -> ControllerId {
        self.id
    }

    pub const fn intent_body(&self) -> Entity {
        self.intent_body
    }

    pub const fn domain(&self) -> IntentDomain {
        self.domain
    }

    pub const fn current_player(&self) -> Option<PlayerId> {
        self.current_player
    }
}

/// In-memory index for Controller runtime identities.
#[derive(Resource, Debug, Default)]
pub struct ControllerRegistry {
    next_controller_id: u64,
    entities_by_id: HashMap<ControllerId, Entity>,
}

impl ControllerRegistry {
    /// Returns the ECS entity currently registered for an ID.
    pub fn controller_entity(&self, controller_id: ControllerId) -> Option<Entity> {
        self.entities_by_id.get(&controller_id).copied()
    }
}

/// Failure to register a Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterControllerError {
    IntentBodyNotFound,
    OverlappingIntentDomain { existing_controller: ControllerId },
}

/// Registers one Controller and atomically checks the target and domain.
///
/// The target must carry `IntentBodyId`. The target is never modified: this
/// external relation preserves the Intent Body's lack of Controller knowledge.
pub fn register_controller(
    world: &mut World,
    intent_body: Entity,
    domain: IntentDomain,
) -> Result<ControllerId, RegisterControllerError> {
    if world.get::<IntentBodyId>(intent_body).is_none() {
        return Err(RegisterControllerError::IntentBodyNotFound);
    }
    world.init_resource::<ControllerRegistry>();
    let registry = world.resource::<ControllerRegistry>();
    for &entity in registry.entities_by_id.values() {
        let Some(existing) = world.get::<Controller>(entity) else {
            continue;
        };
        if existing.intent_body == intent_body && existing.domain.overlaps(domain) {
            return Err(RegisterControllerError::OverlappingIntentDomain {
                existing_controller: existing.id,
            });
        }
    }

    let id = {
        let mut registry = world.resource_mut::<ControllerRegistry>();
        let id = ControllerId(registry.next_controller_id);
        registry.next_controller_id = registry.next_controller_id.saturating_add(1);
        id
    };
    let entity = world
        .spawn(Controller {
            id,
            intent_body,
            domain,
            current_player: None,
        })
        .id();
    world
        .resource_mut::<ControllerRegistry>()
        .entities_by_id
        .insert(id, entity);
    Ok(id)
}

/// Outcome of a successful acquire request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquireControlOutcome {
    Acquired,
    AlreadyControlled,
}

/// Returns a Controller's immutable routing facts for authorized projections.
pub fn controller_state(
    world: &World,
    controller_id: ControllerId,
) -> Option<(IntentDomain, Option<PlayerId>)> {
    let entity = controller_entity(world, controller_id).ok()?;
    let controller = world.get::<Controller>(entity)?;
    Some((controller.domain, controller.current_player))
}

/// Returns the unique Intent Body targeted by a live Controller.
///
/// The caller may route an already authorized input to this entity, but must not
/// attach a reverse Controller reference to the Intent Body.
pub fn controller_intent_body(world: &World, controller_id: ControllerId) -> Option<Entity> {
    let entity = controller_entity(world, controller_id).ok()?;
    world.get::<Controller>(entity).map(Controller::intent_body)
}

/// Failure to acquire, release, or submit through a Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerControlError {
    ControllerNotFound,
    PlayerNotFound,
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
    let entity = controller_entity(world, controller_id)?;
    require_read_write_access(world, player_id, controller_id)?;
    let mut controller = world
        .get_mut::<Controller>(entity)
        .expect("registered Controller entity exists");
    match controller.current_player {
        None => {
            controller.current_player = Some(player_id);
            Ok(AcquireControlOutcome::Acquired)
        }
        Some(current) if current == player_id => Ok(AcquireControlOutcome::AlreadyControlled),
        Some(_) => Err(ControllerControlError::ControlledByOther {
            controller: controller_id,
        }),
    }
}

/// Releases control only when `player_id` is the current controller.
pub fn release_control(
    world: &mut World,
    player_id: PlayerId,
    controller_id: ControllerId,
) -> Result<(), ControllerControlError> {
    let entity = controller_entity(world, controller_id)?;
    let mut controller = world
        .get_mut::<Controller>(entity)
        .expect("registered Controller entity exists");
    if controller.current_player != Some(player_id) {
        return Err(ControllerControlError::NotCurrentController {
            controller: controller_id,
        });
    }
    controller.current_player = None;
    Ok(())
}

/// Validates current control and domain membership before admitting one input.
///
/// The returned input is intentionally not applied to the Intent Body here;
/// concrete Controller behavior and routing remain composition concerns.
pub fn submit_input(
    world: &World,
    player_id: PlayerId,
    controller_id: ControllerId,
    input: ControllerInput,
) -> Result<ControllerInput, ControllerControlError> {
    let entity = controller_entity(world, controller_id)?;
    let controller = world
        .get::<Controller>(entity)
        .expect("registered Controller entity exists");
    if controller.current_player != Some(player_id) {
        return Err(ControllerControlError::NotCurrentController {
            controller: controller_id,
        });
    }
    if !controller.domain.contains(input.kind()) {
        return Err(ControllerControlError::InputOutsideDomain { kind: input.kind() });
    }
    require_read_write_access(world, player_id, controller_id)?;
    Ok(input)
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

fn require_read_write_access(
    world: &World,
    player_id: PlayerId,
    controller_id: ControllerId,
) -> Result<(), ControllerControlError> {
    let Some(registry) = world.get_resource::<PlayerRegistry>() else {
        return Err(ControllerControlError::PlayerNotFound);
    };
    let Some(player_entity) = registry.player_entity(player_id) else {
        return Err(ControllerControlError::PlayerNotFound);
    };
    let Some(player) = world.get::<Player>(player_entity) else {
        return Err(ControllerControlError::PlayerNotFound);
    };
    let level = player.controller_access(controller_id);
    if level != ControllerAccessLevel::ReadWrite {
        return Err(ControllerControlError::AccessDenied { level });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use roundo_creature::IntentBodyBundle;
    use roundo_player::{ExternalIdentityKey, find_or_create_player};

    fn player(world: &mut World, key: &str) -> PlayerId {
        find_or_create_player(world, ExternalIdentityKey::new(key).unwrap())
    }

    fn grant(world: &mut World, player_id: PlayerId, controller_id: ControllerId) {
        let entity = world
            .resource::<PlayerRegistry>()
            .player_entity(player_id)
            .unwrap();
        world
            .get_mut::<Player>(entity)
            .unwrap()
            .set_controller_access(controller_id, ControllerAccessLevel::ReadWrite);
    }

    #[test]
    fn registration_requires_an_intent_body_and_disjoint_domain_per_body() {
        let mut world = World::new();
        let not_an_intent = world.spawn_empty().id();
        assert_eq!(
            register_controller(&mut world, not_an_intent, IntentDomain::movement()),
            Err(RegisterControllerError::IntentBodyNotFound)
        );

        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let movement = register_controller(&mut world, intent, IntentDomain::movement()).unwrap();
        let gaze = register_controller(&mut world, intent, IntentDomain::gaze()).unwrap();
        assert_ne!(movement, gaze);
        assert_eq!(
            register_controller(&mut world, intent, IntentDomain::movement()),
            Err(RegisterControllerError::OverlappingIntentDomain {
                existing_controller: movement
            })
        );
    }

    #[test]
    fn same_domain_is_allowed_for_different_intent_bodies() {
        let mut world = World::new();
        let first = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let second = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(2)))
            .id();
        assert!(register_controller(&mut world, first, IntentDomain::movement()).is_ok());
        assert!(register_controller(&mut world, second, IntentDomain::movement()).is_ok());
    }

    #[test]
    fn acquire_requires_read_write_and_is_non_preemptive_and_idempotent() {
        let mut world = World::new();
        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let controller = register_controller(&mut world, intent, IntentDomain::movement()).unwrap();
        let read_only = player(&mut world, "read");
        let no_access = player(&mut world, "none");
        let owner = player(&mut world, "owner");
        let contender = player(&mut world, "contender");
        let read_entity = world
            .resource::<PlayerRegistry>()
            .player_entity(read_only)
            .unwrap();
        world
            .get_mut::<Player>(read_entity)
            .unwrap()
            .set_controller_access(controller, ControllerAccessLevel::Read);
        grant(&mut world, owner, controller);
        grant(&mut world, contender, controller);

        assert_eq!(
            acquire_control(&mut world, read_only, controller),
            Err(ControllerControlError::AccessDenied {
                level: ControllerAccessLevel::Read
            })
        );
        assert_eq!(
            acquire_control(&mut world, no_access, controller),
            Err(ControllerControlError::AccessDenied {
                level: ControllerAccessLevel::None
            })
        );
        assert_eq!(
            acquire_control(&mut world, PlayerId(999), controller),
            Err(ControllerControlError::PlayerNotFound)
        );
        assert_eq!(
            acquire_control(&mut world, owner, controller),
            Ok(AcquireControlOutcome::Acquired)
        );
        assert_eq!(
            acquire_control(&mut world, owner, controller),
            Ok(AcquireControlOutcome::AlreadyControlled)
        );
        assert_eq!(
            acquire_control(&mut world, contender, controller),
            Err(ControllerControlError::ControlledByOther { controller })
        );
    }

    #[test]
    fn release_and_input_require_current_control_and_domain_membership() {
        let mut world = World::new();
        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let controller = register_controller(&mut world, intent, IntentDomain::movement()).unwrap();
        let owner = player(&mut world, "owner");
        let other = player(&mut world, "other");
        grant(&mut world, owner, controller);
        assert_eq!(
            release_control(&mut world, other, controller),
            Err(ControllerControlError::NotCurrentController { controller })
        );
        acquire_control(&mut world, owner, controller).unwrap();
        assert_eq!(
            submit_input(
                &world,
                other,
                controller,
                ControllerInput::Movement {
                    local_axis: [1.0, 0.0, 0.0]
                }
            ),
            Err(ControllerControlError::NotCurrentController { controller })
        );
        assert_eq!(
            submit_input(
                &world,
                owner,
                controller,
                ControllerInput::Gaze {
                    yaw_delta: 1.0,
                    pitch_delta: 0.0
                }
            ),
            Err(ControllerControlError::InputOutsideDomain {
                kind: IntentKind::Gaze
            })
        );
        assert_eq!(
            submit_input(
                &world,
                owner,
                controller,
                ControllerInput::Movement {
                    local_axis: [1.0, 0.0, 0.0]
                }
            ),
            Ok(ControllerInput::Movement {
                local_axis: [1.0, 0.0, 0.0]
            })
        );
        let owner_entity = world
            .resource::<PlayerRegistry>()
            .player_entity(owner)
            .unwrap();
        world
            .get_mut::<Player>(owner_entity)
            .unwrap()
            .set_controller_access(controller, ControllerAccessLevel::None);
        assert_eq!(
            submit_input(
                &world,
                owner,
                controller,
                ControllerInput::Movement {
                    local_axis: [1.0, 0.0, 0.0]
                }
            ),
            Err(ControllerControlError::AccessDenied {
                level: ControllerAccessLevel::None
            })
        );
        release_control(&mut world, owner, controller).unwrap();
        assert_eq!(
            submit_input(
                &world,
                owner,
                controller,
                ControllerInput::Movement {
                    local_axis: [1.0, 0.0, 0.0]
                }
            ),
            Err(ControllerControlError::NotCurrentController { controller })
        );
    }

    #[test]
    fn zero_access_controller_remains_registered() {
        let mut world = World::new();
        let intent = world
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let controller = register_controller(&mut world, intent, IntentDomain::gaze()).unwrap();
        let entity = world
            .resource::<ControllerRegistry>()
            .controller_entity(controller)
            .unwrap();
        assert!(world.get::<Controller>(entity).is_some());
    }
}
