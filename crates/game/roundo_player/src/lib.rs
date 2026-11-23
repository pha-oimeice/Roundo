//! World-local Player identities and Controller Access.

use bevy::prelude::{Component, Entity, Resource, World};
use roundo_contracts::{ControllerAccessLevel, ControllerId, PlayerId};
use std::collections::HashMap;

/// Non-empty key for finding a local Player.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlayerKey(String);

impl PlayerKey {
    /// Creates a non-empty opaque identity key.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentityKeyError::Empty);
        }
        Ok(Self(value))
    }
}

/// Invalid external identity-key input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityKeyError {
    Empty,
}

/// One authoritative, non-spatial game-world identity and its Controller Access truth.
///
/// Each value belongs to exactly one ECS entity. [`PlayerRegistry`] owns the
/// identity-to-entity index, while this component owns the Player's mutable
/// domain data without requiring a `Transform` or Presence projection.
#[derive(Component, Debug, Eq, PartialEq)]
pub struct Player {
    id: PlayerId,
    controller_access: HashMap<ControllerId, ControllerAccessLevel>,
    access_revision: HashMap<ControllerId, Option<u64>>,
}

impl Player {
    fn new(id: PlayerId) -> Self {
        Self {
            id,
            controller_access: HashMap::new(),
            access_revision: HashMap::new(),
        }
    }

    pub fn id(&self) -> PlayerId {
        self.id
    }

    /// Returns the authorization level for a Controller, defaulting to None.
    pub fn controller_access(&self, controller_id: ControllerId) -> ControllerAccessLevel {
        self.controller_access
            .get(&controller_id)
            .copied()
            .unwrap_or_default()
    }

    /// Monotonic revocation stamp; None permanently denies access on exhaustion.
    pub fn controller_access_revision(&self, controller_id: ControllerId) -> Option<u64> {
        self.access_revision
            .get(&controller_id)
            .copied()
            .unwrap_or(Some(0))
    }

    /// Sets this Player's Controller Access. `None` removes the stored entry.
    pub fn set_controller_access(
        &mut self,
        controller_id: ControllerId,
        level: ControllerAccessLevel,
    ) {
        if self.controller_access(controller_id) == ControllerAccessLevel::ReadWrite
            && level != ControllerAccessLevel::ReadWrite
        {
            let next = self
                .controller_access_revision(controller_id)
                .and_then(|n| n.checked_add(1));
            self.access_revision.insert(controller_id, next);
        }
        match level {
            ControllerAccessLevel::None => {
                self.controller_access.remove(&controller_id);
            }
            _ => {
                self.controller_access.insert(controller_id, level);
            }
        }
    }

    /// Iterates the explicitly granted Controller Access entries in no order.
    pub fn controller_accesses(
        &self,
    ) -> impl Iterator<Item = (ControllerId, ControllerAccessLevel)> + '_ {
        self.controller_access
            .iter()
            .map(|(&id, &level)| (id, level))
    }
}

#[derive(Resource, Debug)]
pub struct PlayerRegistry {
    next_player_id: u64,
    entities_by_id: HashMap<PlayerId, Entity>,
    players_by_identity: HashMap<PlayerKey, PlayerId>,
}

impl Default for PlayerRegistry {
    fn default() -> Self {
        Self {
            next_player_id: 1,
            entities_by_id: HashMap::new(),
            players_by_identity: HashMap::new(),
        }
    }
}

impl PlayerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the authoritative ECS entity for one live Player identity.
    pub fn player_entity(&self, player_id: PlayerId) -> Option<Entity> {
        self.entities_by_id.get(&player_id).copied()
    }

    /// Removes every registry association for an ECS entity whose `Player`
    /// component is being removed.
    pub fn remove_player_entity(&mut self, entity: Entity) -> Option<PlayerId> {
        let player_id = self
            .entities_by_id
            .iter()
            .find_map(|(&id, &candidate)| (candidate == entity).then_some(id))?;
        self.entities_by_id.remove(&player_id);
        self.players_by_identity
            .retain(|_, candidate| *candidate != player_id);
        Some(player_id)
    }
}

/// Resolves one local key to its stable in-process Player, atomically
/// spawning the authoritative non-spatial ECS entity on first use.
///
/// This exclusive-World interface keeps identity allocation, ECS creation, and
/// registry publication in one module so callers cannot register an arbitrary
/// entity as a Player. It initializes the in-memory registry lazily.
pub fn find_or_create_player(world: &mut World, identity: PlayerKey) -> PlayerId {
    world.init_resource::<PlayerRegistry>();
    if let Some(&player_id) = world
        .resource::<PlayerRegistry>()
        .players_by_identity
        .get(&identity)
    {
        return player_id;
    }

    let player_id = {
        let mut registry = world.resource_mut::<PlayerRegistry>();
        let player_id = PlayerId(registry.next_player_id);
        registry.next_player_id = registry.next_player_id.saturating_add(1);
        player_id
    };
    let entity = world.spawn(Player::new(player_id)).id();
    let mut registry = world.resource_mut::<PlayerRegistry>();
    registry.entities_by_id.insert(player_id, entity);
    registry.players_by_identity.insert(identity, player_id);
    player_id
}
