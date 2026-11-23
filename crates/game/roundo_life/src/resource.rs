//! Life's private data adapter. DSA owns lifetime and grants; Life owns mutation.
//! Neither payload locks, Owners nor issuing endpoints reach Body callers.

use bevy::prelude::{Resource, World};
use roundo_infrastructure::dsa::data_structure::resource_access::{
    AccessError, ModifyAccess, Owner, ReadAccess, ResourceAccessManager, Shared,
};
use std::sync::RwLock;

pub(crate) type ReadGrant<T> = ReadAccess<RwLock<T>, Shared>;
pub(crate) type ModifyGrant<T> = ModifyAccess<RwLock<T>, Shared>;

pub(crate) struct Registration<A> {
    pub(crate) generation: u64,
    pub(crate) access: A,
}

#[derive(Resource)]
pub(crate) struct DataResources(ResourceAccessManager<Shared>);
impl Default for DataResources {
    fn default() -> Self {
        Self(ResourceAccessManager::shared())
    }
}

pub(crate) struct ManagedData<T: Send + Sync + 'static> {
    owner: Owner<RwLock<T>, Shared>,
    // Trusted adapter-only access, never a Body's implicit read permission.
    internal_read: ReadGrant<T>,
}

impl<T: Send + Sync + 'static> ManagedData<T> {
    pub(crate) fn create(world: &mut World, value: T) -> Result<Self, AccessError> {
        let mut owner = world
            .resource::<DataResources>()
            .0
            .create(RwLock::new(value))
            .map_err(|error| error.error)?;
        let internal_read = owner.grant_read()?;
        Ok(Self {
            owner,
            internal_read,
        })
    }

    pub(crate) fn grant_read(&mut self) -> Result<ReadGrant<T>, AccessError> {
        self.owner.grant_read()
    }
    pub(crate) fn grant_modify(&mut self) -> Result<ModifyGrant<T>, AccessError> {
        self.owner.grant_modify()
    }
    pub(crate) fn revoke_read(&mut self, access: &ReadGrant<T>) {
        self.owner
            .revoke(&access.grant_id())
            .expect("Life stores exact resource grants");
    }
    pub(crate) fn revoke_modify(&mut self, access: &ModifyGrant<T>) {
        self.owner
            .revoke(&access.grant_id())
            .expect("Life stores exact resource grants");
    }
    pub(crate) fn read<R>(
        &self,
        access: &ReadGrant<T>,
        operation: impl FnOnce(&T) -> R,
    ) -> Result<R, AccessError> {
        self.owner.protect(|scope| {
            scope.with_read(access, |data| {
                operation(&data.read().unwrap_or_else(|error| error.into_inner()))
            })
        })?
    }
    pub(crate) fn inspect<R>(&self, operation: impl FnOnce(&T) -> R) -> R {
        self.read(&self.internal_read, operation)
            .expect("private Life resource is live and non-reentrant")
    }
    pub(crate) fn modify<R>(
        &self,
        access: &ModifyGrant<T>,
        operation: impl FnOnce(&mut T) -> R,
    ) -> Result<R, AccessError> {
        self.owner.protect(|scope| {
            scope.with_modify_permission(access, |_| ())?;
            // Validation and mutation share one Owner protection: no grant change
            // or destruction can intervene. Keep read and modify scope operations
            // sequential, not recursively nested. Mutation itself belongs to Life.
            scope.with_read(&self.internal_read, |data| {
                operation(&mut data.write().unwrap_or_else(|error| error.into_inner()))
            })
        })?
    }
    pub(crate) fn maintain<R>(&mut self, operation: impl FnOnce(&mut T) -> R) -> R {
        self.owner
            .protect(|scope| {
                scope.with_read(&self.internal_read, |data| {
                    operation(&mut data.write().unwrap_or_else(|error| error.into_inner()))
                })
            })
            .expect("exclusive Life maintenance")
            .expect("private maintenance grant")
    }
}
