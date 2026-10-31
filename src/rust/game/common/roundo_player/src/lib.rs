//! Non-spatial Player identity, connection association, and Controller Access.
//!
//! This crate owns authoritative non-spatial Player ECS entities and their
//! in-memory identity registry. It deliberately has no transport, login,
//! Presence, Life, or Controller implementation dependency. A transport/login
//! adapter resolves an opaque external identity before calling
//! [`find_or_create_player`].

use bevy::prelude::{Component, Entity, Resource, World};
use roundo_contracts::{ConnectionId, ControllerAccessLevel, ControllerId, PlayerId};
use std::collections::HashMap;

/// Opaque external identity resolved by a transport or future login adapter.
///
/// Its value has no Player-domain semantics. In particular, a temporary peer
/// address may be encoded here without becoming a `PlayerId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalIdentityKey(String);

impl ExternalIdentityKey {
    /// Creates a non-empty opaque identity key.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentityKeyError::Empty);
        }
        Ok(Self(value))
    }

    /// Exposes the opaque value only to the identity-adapter boundary.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Invalid external identity-key input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityKeyError {
    Empty,
}

/// Seam implemented by transport or login adapters before Player lookup.
///
/// The Player domain never interprets the source or the resulting key.
pub trait ExternalIdentityProvider<Source> {
    type Error;

    fn resolve(&self, source: &Source) -> Result<ExternalIdentityKey, Self::Error>;
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

/// In-memory Player identity and connection-association registry.
///
/// Players and their Access entries outlive connection bindings. The registry
/// makes no policy decision about how many simultaneous connections one Player
/// may have; it only ensures a single connection maps to at most one Player.
#[derive(Resource, Debug)]
pub struct PlayerRegistry {
    next_player_id: u64,
    entities_by_id: HashMap<PlayerId, Entity>,
    players_by_identity: HashMap<ExternalIdentityKey, PlayerId>,
    players_by_connection: HashMap<ConnectionId, PlayerId>,
}

impl Default for PlayerRegistry {
    fn default() -> Self {
        Self {
            next_player_id: 1,
            entities_by_id: HashMap::new(),
            players_by_identity: HashMap::new(),
            players_by_connection: HashMap::new(),
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

    /// Associates an unbound connection with an existing Player.
    pub fn bind_connection(
        &mut self,
        connection_id: ConnectionId,
        player_id: PlayerId,
    ) -> Result<(), ConnectionBindingError> {
        if !self.entities_by_id.contains_key(&player_id) {
            return Err(ConnectionBindingError::PlayerNotFound);
        }
        if self.players_by_connection.contains_key(&connection_id) {
            return Err(ConnectionBindingError::AlreadyBound);
        }
        self.players_by_connection.insert(connection_id, player_id);
        Ok(())
    }

    /// Removes only the transient connection association.
    pub fn unbind_connection(&mut self, connection_id: ConnectionId) -> Option<PlayerId> {
        self.players_by_connection.remove(&connection_id)
    }

    pub fn player_for_connection(&self, connection_id: ConnectionId) -> Option<PlayerId> {
        self.players_by_connection.get(&connection_id).copied()
    }

    /// Removes every registry association for an ECS entity whose `Player`
    /// component is being removed.
    ///
    /// This is the lifecycle seam used by Controller cleanup. Ordinary
    /// disconnects must use [`Self::unbind_connection`] instead and therefore
    /// do not destroy the Player identity or its durable runtime state.
    pub fn remove_player_entity(&mut self, entity: Entity) -> Option<PlayerId> {
        let player_id = self
            .entities_by_id
            .iter()
            .find_map(|(&id, &candidate)| (candidate == entity).then_some(id))?;
        self.entities_by_id.remove(&player_id);
        self.players_by_identity
            .retain(|_, candidate| *candidate != player_id);
        self.players_by_connection
            .retain(|_, candidate| *candidate != player_id);
        Some(player_id)
    }

    /// Iterates every current connection for one Player without defining a
    /// multi-connection login policy.
    pub fn connections_for_player(
        &self,
        player_id: PlayerId,
    ) -> impl Iterator<Item = ConnectionId> + '_ {
        self.players_by_connection
            .iter()
            .filter_map(move |(&connection, &owner)| (owner == player_id).then_some(connection))
    }
}

/// Resolves one external identity to its stable in-process Player, atomically
/// spawning the authoritative non-spatial ECS entity on first use.
///
/// This exclusive-World interface keeps identity allocation, ECS creation, and
/// registry publication in one module so callers cannot register an arbitrary
/// entity as a Player. It initializes the in-memory registry lazily.
pub fn find_or_create_player(world: &mut World, identity: ExternalIdentityKey) -> PlayerId {
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

/// Failure while associating a transient connection to a Player.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionBindingError {
    PlayerNotFound,
    AlreadyBound,
}
