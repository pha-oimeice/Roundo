//! ID-based native access with explicit, registered-only ECS borrow declarations.

use super::{
    AttributeAccessError, AttributeDescriptor, AttributeId, AttributeRegistry, Unorm,
    registry::Domain,
};
use bevy::{
    ecs::{
        change_detection::Tick,
        query::{FilteredAccessSet, QueryBuilder},
        system::{ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamValidationError},
        world::{
            FilteredEntityMut, FilteredEntityRef, WorldId, unsafe_world_cell::UnsafeWorldCell,
        },
    },
    prelude::{Entity, Query, Res, World},
};
use std::any::Any;

/// A shared native payload and its registered type information. No clone or
/// allocation is needed to read. The reference cannot outlive the ECS borrow.
pub struct AttributeRef<'a> {
    descriptor: &'a AttributeDescriptor,
    value: &'a (dyn Any + Send + Sync),
}
impl<'a> AttributeRef<'a> {
    pub fn descriptor(&self) -> &AttributeDescriptor {
        self.descriptor
    }
    pub fn as_any(&self) -> &'a (dyn Any + Send + Sync) {
        self.value
    }
    pub fn downcast_ref<T: 'static>(&self) -> Option<&'a T> {
        self.value.downcast_ref()
    }
    pub fn as_i32(&self) -> Option<i32> {
        self.downcast_ref::<i32>().copied()
    }
    pub fn as_unorm(&self) -> Option<Unorm> {
        self.downcast_ref::<Unorm>().copied()
    }
}

// The registry is frozen before any of these queries initialize. Dynamic
// optional accesses retain ECS composition: an entity need not have every
// registered attribute. No wildcard/all-component borrow is requested.
macro_rules! registered_query {
    ($name:ident, $data:ident, $access:ident) => {
        struct $name<'w, 's> {
            query: Query<'w, 's, $data<'static, 'static>>,
            domain: Domain,
        }
        // SAFETY: The query state is built in the initialization World and is
        // never expanded afterward. init_access delegates the exact filtered
        // accesses to Bevy (which rejects conflicts); get_param delegates the
        // same state, World, ticks and lifetimes without introducing any access.
        unsafe impl SystemParam for $name<'_, '_> {
            type State = (
                <Query<'static, 'static, $data<'static, 'static>> as SystemParam>::State,
                Domain,
            );
            type Item<'w, 's> = $name<'w, 's>;
            fn init_state(world: &mut World) -> Self::State {
                let registry = world.get_resource::<AttributeRegistry>().expect(
                    "AttributeRegistry must be installed before attribute systems initialize",
                );
                let domain = registry.initialized_domain(world.id());
                let components: Vec<_> = registry.components().collect();
                let mut builder = QueryBuilder::<$data>::new(world);
                builder.optional(|optional| {
                    for component in &components {
                        optional.$access(*component);
                    }
                });
                (builder.build(), domain)
            }
            fn init_access(
                state: &Self::State,
                meta: &mut SystemMeta,
                access: &mut FilteredAccessSet,
                world: &mut World,
            ) {
                <Query<$data> as SystemParam>::init_access(&state.0, meta, access, world);
            }
            unsafe fn get_param<'w, 's>(
                state: &'s mut Self::State,
                meta: &SystemMeta,
                world: UnsafeWorldCell<'w>,
                tick: Tick,
            ) -> Result<Self::Item<'w, 's>, SystemParamValidationError> {
                // SAFETY: init_access registered this exact QueryState's access;
                // SystemParam's caller supplies its matching World and borrows.
                let query = unsafe {
                    <Query<$data> as SystemParam>::get_param(&mut state.0, meta, world, tick)?
                };
                Ok($name {
                    query,
                    domain: state.1,
                })
            }
        }
    };
}
registered_query!(RegisteredReads, FilteredEntityRef, ref_id);
registered_query!(RegisteredWrites, FilteredEntityMut, mut_id);
// SAFETY: RegisteredReads delegates exclusively to a read-only filtered query.
// Its runtime methods cannot mutate components or resources.
unsafe impl ReadOnlySystemParam for RegisteredReads<'_, '_> {}

/// Read-only access to registered attributes on trusted local Entity targets.
///
/// All registered component reads are visible to the scheduler; unrelated
/// components/resources remain compatible. IDs and Entities do not grant Player
/// access, and a bare Entity is a local selector, not a cross-World credential.
///
///
/// Borrowed payloads cannot escape the parameter's access lifetime:
#[derive(SystemParam)]
pub struct AttributeReads<'w, 's> {
    registry: Res<'w, AttributeRegistry>,
    world: WorldId,
    entities: RegisteredReads<'w, 's>,
}
impl AttributeReads<'_, '_> {
    pub fn describe(&self, id: AttributeId) -> Result<&AttributeDescriptor, AttributeAccessError> {
        Ok(self
            .registry
            .checked_entry(self.world, self.entities.domain, id)?
            .descriptor())
    }
    pub fn get(
        &self,
        entity: Entity,
        id: AttributeId,
    ) -> Result<Option<AttributeRef<'_>>, AttributeAccessError> {
        let entry = self
            .registry
            .checked_entry(self.world, self.entities.domain, id)?;
        let target = self
            .entities
            .query
            .get(entity)
            .map_err(|_| AttributeAccessError::EntityNotFound)?;
        Ok(entry.read(target).map(|value| AttributeRef {
            descriptor: entry.descriptor(),
            value,
        }))
    }
}

/// ID-based updates to existing registered native components, not structural
/// insertion/removal or Player authorization. Writes are synchronous.
///
/// Declares mutable access to the frozen registry's entire component set, not
/// merely the IDs eventually used by this invocation. This conservatively
/// serializes registered-attribute writers, but not unrelated component systems.
/// For hot paths with narrower needs, ordinary typed Queries remain available.
#[derive(SystemParam)]
pub struct AttributeWrites<'w, 's> {
    registry: Res<'w, AttributeRegistry>,
    world: WorldId,
    entities: RegisteredWrites<'w, 's>,
}
impl AttributeWrites<'_, '_> {
    pub fn describe(&self, id: AttributeId) -> Result<&AttributeDescriptor, AttributeAccessError> {
        Ok(self
            .registry
            .checked_entry(self.world, self.entities.domain, id)?
            .descriptor())
    }
    pub fn get(
        &self,
        entity: Entity,
        id: AttributeId,
    ) -> Result<Option<AttributeRef<'_>>, AttributeAccessError> {
        let entry = self
            .registry
            .checked_entry(self.world, self.entities.domain, id)?;
        let target = self
            .entities
            .query
            .get(entity)
            .map_err(|_| AttributeAccessError::EntityNotFound)?;
        Ok(entry.read(target).map(|value| AttributeRef {
            descriptor: entry.descriptor(),
            value,
        }))
    }
    /// Checks registry/ID, target and component presence, then payload type and
    /// adapter validation before taking a mutable component reference. Rejection
    /// does not replace or mark the component changed for the built-in adapters.
    /// The input is borrowed; only the adapter's native value clone is consumed.
    pub fn set(
        &mut self,
        entity: Entity,
        id: AttributeId,
        value: &(dyn Any + Send + Sync),
    ) -> Result<(), AttributeAccessError> {
        let entry = self
            .registry
            .checked_entry(self.world, self.entities.domain, id)?;
        let mut target = self
            .entities
            .query
            .get_mut(entity)
            .map_err(|_| AttributeAccessError::EntityNotFound)?;
        entry.write(&mut target, value)
    }
}
