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
}

impl Player {
    fn new(id: PlayerId) -> Self {
        Self {
            id,
            controller_access: HashMap::new(),
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

    /// Sets this Player's Controller Access. `None` removes the stored entry.
    pub fn set_controller_access(
        &mut self,
        controller_id: ControllerId,
        level: ControllerAccessLevel,
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Transform;

    fn identity(value: &str) -> ExternalIdentityKey {
        ExternalIdentityKey::new(value).unwrap()
    }

    fn find_or_create(world: &mut World, value: &str) -> PlayerId {
        find_or_create_player(world, identity(value))
    }

    fn player(world: &World, player_id: PlayerId) -> &Player {
        let entity = world
            .resource::<PlayerRegistry>()
            .player_entity(player_id)
            .unwrap();
        world.get::<Player>(entity).unwrap()
    }

    #[test]
    fn external_identity_is_opaque_and_non_empty() {
        assert_eq!(
            ExternalIdentityKey::new("").unwrap_err(),
            IdentityKeyError::Empty
        );
        assert_eq!(
            identity("peer:127.0.0.1:1234").as_str(),
            "peer:127.0.0.1:1234"
        );
    }

    #[test]
    fn find_or_create_spawns_one_non_spatial_entity_per_identity() {
        let mut world = World::new();
        let first = find_or_create(&mut world, "peer:a");
        let again = find_or_create(&mut world, "peer:a");
        let second = find_or_create(&mut world, "peer:b");

        assert_eq!(first, again);
        assert_ne!(first, second);
        assert_eq!(player(&world, first).id(), first);
        assert_eq!(world.query::<&Player>().iter(&world).count(), 2);
        let first_entity = world
            .resource::<PlayerRegistry>()
            .player_entity(first)
            .unwrap();
        assert!(world.get::<Transform>(first_entity).is_none());
    }

    #[test]
    fn connection_unbinding_keeps_player_entity_and_access() {
        let mut world = World::new();
        let player_id = find_or_create(&mut world, "peer:a");
        let player_entity = world
            .resource::<PlayerRegistry>()
            .player_entity(player_id)
            .unwrap();
        let controller_id = ControllerId(9);
        world
            .get_mut::<Player>(player_entity)
            .unwrap()
            .set_controller_access(controller_id, ControllerAccessLevel::ReadWrite);

        world
            .resource_mut::<PlayerRegistry>()
            .bind_connection(ConnectionId(4), player_id)
            .unwrap();
        assert_eq!(
            world
                .resource_mut::<PlayerRegistry>()
                .unbind_connection(ConnectionId(4)),
            Some(player_id)
        );
        assert_eq!(
            world
                .resource::<PlayerRegistry>()
                .player_for_connection(ConnectionId(4)),
            None
        );
        assert_eq!(
            player(&world, player_id).controller_access(controller_id),
            ControllerAccessLevel::ReadWrite
        );
    }

    #[test]
    fn player_may_have_multiple_connections_without_selecting_a_login_policy() {
        let mut world = World::new();
        let player_id = find_or_create(&mut world, "peer:a");
        let mut registry = world.resource_mut::<PlayerRegistry>();

        registry
            .bind_connection(ConnectionId(1), player_id)
            .unwrap();
        registry
            .bind_connection(ConnectionId(2), player_id)
            .unwrap();

        assert_eq!(
            registry.player_for_connection(ConnectionId(1)),
            Some(player_id)
        );
        assert_eq!(
            registry.player_for_connection(ConnectionId(2)),
            Some(player_id)
        );
    }

    #[test]
    fn binding_rejects_unknown_players_and_rebinding_one_connection() {
        let mut world = World::new();
        world.init_resource::<PlayerRegistry>();
        assert_eq!(
            world
                .resource_mut::<PlayerRegistry>()
                .bind_connection(ConnectionId(1), PlayerId(99)),
            Err(ConnectionBindingError::PlayerNotFound)
        );

        let player_id = find_or_create(&mut world, "peer:a");
        let mut registry = world.resource_mut::<PlayerRegistry>();
        registry
            .bind_connection(ConnectionId(1), player_id)
            .unwrap();
        assert_eq!(
            registry.bind_connection(ConnectionId(1), player_id),
            Err(ConnectionBindingError::AlreadyBound)
        );
    }

    #[test]
    fn none_access_is_absent_and_read_access_is_preserved() {
        let mut world = World::new();
        let player_id = find_or_create(&mut world, "peer:a");
        let player_entity = world
            .resource::<PlayerRegistry>()
            .player_entity(player_id)
            .unwrap();
        let controller = ControllerId(5);
        let mut player = world.get_mut::<Player>(player_entity).unwrap();

        player.set_controller_access(controller, ControllerAccessLevel::Read);
        assert_eq!(
            player.controller_access(controller),
            ControllerAccessLevel::Read
        );
        assert_eq!(
            player.controller_accesses().collect::<Vec<_>>(),
            vec![(controller, ControllerAccessLevel::Read)]
        );

        player.set_controller_access(controller, ControllerAccessLevel::None);
        assert_eq!(
            player.controller_access(controller),
            ControllerAccessLevel::None
        );
        assert!(player.controller_accesses().next().is_none());
    }
}
