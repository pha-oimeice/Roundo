use crate::NetworkError;
use bevy::prelude::{Entity, Resource};
use roundo_toolbox::id_generator::IdGenerator;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Shared data identity, distinct from either World's native Entity value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SharedEntityId(pub u64);

/// Server-owned index for the explicitly shared portion of a World.
///
/// Install with an explicit first ID. The caller decides which native entities
/// to associate and when to remove an association. No remote entity creation,
/// lifecycle protocol, or automatic identity recycling is implied.
#[derive(Resource, Debug)]
pub struct SharedEntities {
    entities: HashMap<SharedEntityId, Entity>,
    generator: IdGenerator<u64>,
}

impl SharedEntities {
    pub fn new(first_id: SharedEntityId) -> Self {
        Self {
            entities: HashMap::new(),
            generator: IdGenerator::new(first_id.0),
        }
    }

    /// Associates an entity supplied by this World's caller. This index does
    /// not keep the entity alive or inspect/despawn it on behalf of the caller.
    pub fn share(&mut self, entity: Entity) -> Result<SharedEntityId, NetworkError> {
        let id = SharedEntityId(
            self.generator
                .get_next()
                .ok_or(NetworkError::IdentityExhausted)?,
        );
        self.entities.insert(id, entity);
        Ok(id)
    }

    pub fn entity(&self, id: SharedEntityId) -> Option<Entity> {
        self.entities.get(&id).copied()
    }

    pub fn remove(&mut self, id: SharedEntityId) -> Option<Entity> {
        self.entities.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (SharedEntityId, Entity)> + '_ {
        self.entities.iter().map(|(&id, &entity)| (id, entity))
    }
}
