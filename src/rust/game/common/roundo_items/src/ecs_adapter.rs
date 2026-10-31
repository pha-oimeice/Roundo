//! Scheduler-visible item storage and trusted host publication.

use crate::{
    DeclaredItemData, ItemAccessError, ItemDataStore, ItemDefinitions, ItemInstallError,
    ItemModifyAccess, ItemReadAccess, ItemRuntimeSetup, ItemSchema, SpecializedData,
    access::ItemAccessTarget,
};
use bevy::{
    ecs::{system::SystemParam, world::WorldId},
    prelude::*,
};
use roundo_dsa::data_structure::resource_access::{
    AccessError, Owner, ResourceAccessManager, Shared,
};
use roundo_inventory::ItemId;
use roundo_mod_loader::ResourceName;
use std::collections::HashMap;

#[derive(Resource)]
struct ItemRuntimeData {
    world: WorldId,
    definitions: ItemDefinitions,
    store: ItemDataStore,
    declared_schema: ItemSchema<DeclaredItemData>,
}

#[derive(Resource)]
struct ItemRuntimeAuthority {
    world: WorldId,
    manager: ResourceAccessManager<Shared>,
    owners: HashMap<ItemId, Owner<ItemAccessTarget, Shared>>,
}

/// Installs empty item capabilities without issuing identities or grants.
///
/// Either existing runtime resource causes rejection without replacement. This
/// adds no gameplay entities or systems and provides no uninstall/reset API.
pub fn install_items(app: &mut App, setup: ItemRuntimeSetup) -> Result<(), ItemInstallError> {
    install_with_manager(app, setup, ResourceAccessManager::shared())
}

fn install_with_manager(
    app: &mut App,
    setup: ItemRuntimeSetup,
    manager: ResourceAccessManager<Shared>,
) -> Result<(), ItemInstallError> {
    if app.world().contains_resource::<ItemRuntimeData>()
        || app.world().contains_resource::<ItemRuntimeAuthority>()
    {
        return Err(ItemInstallError::AlreadyInstalled);
    }
    let (definitions, store, declared_schema) = setup.into_parts();
    let world = app.world().id();
    let data = ItemRuntimeData {
        world,
        definitions,
        store,
        declared_schema,
    };
    let authority = ItemRuntimeAuthority {
        world,
        manager,
        owners: HashMap::new(),
    };
    app.insert_resource(data).insert_resource(authority);
    Ok(())
}

/// Trusted host capability for publishing native data and an authorization target.
///
/// This is not a native-code sandbox. Publication does not issue an ItemId,
/// grant recipient access or complete an Inventory/game-instance transaction.
#[derive(SystemParam)]
pub struct ItemPublisher<'w> {
    world: WorldId,
    data: ResMut<'w, ItemRuntimeData>,
    authority: ResMut<'w, ItemRuntimeAuthority>,
}

impl ItemPublisher<'_> {
    /// Publishes an instance identity with no prototype or mandatory type payload.
    /// Registered Specialized sources can subsequently be composed through ItemWrites.
    /// This creates a data authorization target, not custody or gameplay permission.
    pub fn publish_instance(&mut self, item: ItemId) -> Result<(), ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        if self.authority.owners.contains_key(&item) || self.data.store.type_key(item).is_some() {
            return Err(ItemAccessError::AlreadyPublished);
        }
        let owner = self
            .authority
            .manager
            .create(ItemAccessTarget { item })
            .map_err(|error| ItemAccessError::Access(error.error))?;
        self.authority.owners.insert(item, owner);
        Ok(())
    }

    /// Publishes one known definition under an externally supplied identity.
    ///
    /// Recoverable rejection leaves existing data and targets unchanged. A
    /// subsequent game operation failing does not roll back this publication.
    pub fn publish(
        &mut self,
        item: ItemId,
        definition: &ResourceName,
    ) -> Result<(), ItemAccessError> {
        if self.world != self.data.world || self.world != self.authority.world {
            return Err(ItemAccessError::WrongWorld);
        }
        if self.authority.owners.contains_key(&item) || self.data.store.type_key(item).is_some() {
            return Err(ItemAccessError::AlreadyPublished);
        }
        let payload = self
            .data
            .definitions
            .materialize(definition)
            .map_err(|error| ItemAccessError::UnknownDefinition(error.name))?;
        let owner = self
            .authority
            .manager
            .create(ItemAccessTarget { item })
            .map_err(|error| ItemAccessError::Access(error.error))?;
        let schema = self.data.declared_schema;
        self.data
            .store
            .set_type(schema, item, payload)
            .map_err(ItemAccessError::Data)?;
        self.authority.owners.insert(item, owner);
        Ok(())
    }
}

fn check_world(
    world: WorldId,
    data: &ItemRuntimeData,
    authority: &ItemRuntimeAuthority,
) -> Result<(), ItemAccessError> {
    if world != data.world || world != authority.world {
        return Err(ItemAccessError::WrongWorld);
    }
    Ok(())
}

fn authorized_read<R>(
    world: WorldId,
    data: &ItemRuntimeData,
    authority: &ItemRuntimeAuthority,
    item: ItemId,
    access: &ItemReadAccess,
    read: impl FnOnce(&ItemDataStore) -> Result<R, ItemAccessError>,
) -> Result<R, ItemAccessError> {
    check_world(world, data, authority)?;
    let owner = authority
        .owners
        .get(&item)
        .ok_or(ItemAccessError::UnknownItem)?;
    owner
        .protect(|scope| {
            scope.with_read(&access.0, |marker| {
                if marker.item != item {
                    return Err(ItemAccessError::Access(AccessError::ResourceMismatch));
                }
                read(&data.store)
            })
        })
        .map_err(ItemAccessError::Access)?
        .map_err(ItemAccessError::Access)?
}

/// Shared recipient access; every read still requires an exact Read grant.
///
/// Runtime resources stay private; use the authorized methods instead:
///
///
#[derive(SystemParam)]
pub struct ItemReads<'w> {
    world: WorldId,
    data: Res<'w, ItemRuntimeData>,
    authority: Res<'w, ItemRuntimeAuthority>,
}

impl ItemReads<'_> {
    /// Reads native data only inside the current grant's protected callback.
    ///
    /// Return an owned snapshot; the callback's payload borrow cannot escape:
    ///
    ///
    pub fn with_data<R>(
        &self,
        item: ItemId,
        access: &ItemReadAccess,
        read: impl for<'data> FnOnce(&'data DeclaredItemData) -> R,
    ) -> Result<R, ItemAccessError> {
        authorized_read(
            self.world,
            &self.data,
            &self.authority,
            item,
            access,
            |store| {
                store
                    .with_type(self.data.declared_schema, item, read)
                    .map_err(ItemAccessError::Data)?
                    .ok_or(ItemAccessError::MissingData)
            },
        )
    }

    /// Copies the native table without granting modification permission.
    ///
    /// A Modify grant does not authorize this read:
    ///
    ///
    ///
    /// An ItemId alone is not authorization:
    ///
    ///
    pub fn copy_data(
        &self,
        item: ItemId,
        access: &ItemReadAccess,
    ) -> Result<toml::Table, ItemAccessError> {
        self.with_data(item, access, |value| value.data().clone())
    }

    /// Reads a host source; an absent source returns `Ok(None)`.
    pub fn with_specialized<T: SpecializedData, R>(
        &self,
        schema: ItemSchema<T>,
        item: ItemId,
        access: &ItemReadAccess,
        read: impl for<'data> FnOnce(&'data T) -> R,
    ) -> Result<Option<R>, ItemAccessError> {
        authorized_read(
            self.world,
            &self.data,
            &self.authority,
            item,
            access,
            |store| {
                store
                    .with_specialized(schema, item, read)
                    .map_err(ItemAccessError::Data)
            },
        )
    }
}

/// Recipient modification capability; reading additionally requires a Read grant.
#[derive(SystemParam)]
pub struct ItemWrites<'w> {
    world: WorldId,
    data: ResMut<'w, ItemRuntimeData>,
    authority: Res<'w, ItemRuntimeAuthority>,
}

impl ItemWrites<'_> {
    /// Reads native data with a separate Read grant.
    pub fn with_data<R>(
        &self,
        item: ItemId,
        access: &ItemReadAccess,
        read: impl for<'data> FnOnce(&'data DeclaredItemData) -> R,
    ) -> Result<R, ItemAccessError> {
        authorized_read(
            self.world,
            &self.data,
            &self.authority,
            item,
            access,
            |store| {
                store
                    .with_type(self.data.declared_schema, item, read)
                    .map_err(ItemAccessError::Data)?
                    .ok_or(ItemAccessError::MissingData)
            },
        )
    }

    /// Copies native data with a separate Read grant.
    pub fn copy_data(
        &self,
        item: ItemId,
        access: &ItemReadAccess,
    ) -> Result<toml::Table, ItemAccessError> {
        self.with_data(item, access, |value| value.data().clone())
    }

    /// Reads one host source with a separate Read grant.
    pub fn with_specialized<T: SpecializedData, R>(
        &self,
        schema: ItemSchema<T>,
        item: ItemId,
        access: &ItemReadAccess,
        read: impl for<'data> FnOnce(&'data T) -> R,
    ) -> Result<Option<R>, ItemAccessError> {
        authorized_read(
            self.world,
            &self.data,
            &self.authority,
            item,
            access,
            |store| {
                store
                    .with_specialized(schema, item, read)
                    .map_err(ItemAccessError::Data)
            },
        )
    }

    /// Replaces only native data, preserving definition and Specialized sources.
    ///
    /// A Read grant cannot be used as modification qualification:
    ///
    ///
    pub fn replace_data(
        &mut self,
        item: ItemId,
        access: &ItemModifyAccess,
        data: toml::Table,
    ) -> Result<(), ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        let owner = self
            .authority
            .owners
            .get(&item)
            .ok_or(ItemAccessError::UnknownItem)?;
        owner
            .protect(|scope| {
                scope.with_modify_permission(&access.0, |evidence| {
                    if evidence.resource_id() != owner.id() {
                        return Err(ItemAccessError::Access(AccessError::ResourceMismatch));
                    }
                    let schema = self.data.declared_schema;
                    let replacement = self
                        .data
                        .store
                        .with_type(schema, item, |current| current.with_replaced_data(data))
                        .map_err(ItemAccessError::Data)?
                        .ok_or(ItemAccessError::MissingData)?;
                    self.data
                        .store
                        .set_type(schema, item, replacement)
                        .map_err(ItemAccessError::Data)
                })
            })
            .map_err(ItemAccessError::Access)?
            .map_err(ItemAccessError::Access)?
    }

    /// Commits one host source after current exact-target Modify authorization.
    pub fn set_specialized<T: SpecializedData>(
        &mut self,
        schema: ItemSchema<T>,
        item: ItemId,
        access: &ItemModifyAccess,
        value: T,
    ) -> Result<(), ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        let owner = self
            .authority
            .owners
            .get(&item)
            .ok_or(ItemAccessError::UnknownItem)?;
        owner
            .protect(|scope| {
                scope.with_modify_permission(&access.0, |evidence| {
                    if evidence.resource_id() != owner.id() {
                        return Err(ItemAccessError::Access(AccessError::ResourceMismatch));
                    }
                    self.data
                        .store
                        .set_specialized(schema, item, value)
                        .map_err(ItemAccessError::Data)
                })
            })
            .map_err(ItemAccessError::Access)?
            .map_err(ItemAccessError::Access)?
    }
}

impl ItemWrites<'_> {
    /// Removes one source after exact-target Modify authorization. Other data,
    /// custody and grants are unchanged; an absent source returns false.
    pub fn remove_specialized<T: SpecializedData>(
        &mut self,
        schema: ItemSchema<T>,
        item: ItemId,
        access: &ItemModifyAccess,
    ) -> Result<bool, ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        let owner = self
            .authority
            .owners
            .get(&item)
            .ok_or(ItemAccessError::UnknownItem)?;
        owner
            .protect(|scope| {
                scope.with_modify_permission(&access.0, |evidence| {
                    if evidence.resource_id() != owner.id() {
                        return Err(ItemAccessError::Access(AccessError::ResourceMismatch));
                    }
                    self.data
                        .store
                        .remove_specialized(schema, item)
                        .map_err(ItemAccessError::Data)
                })
            })
            .map_err(ItemAccessError::Access)?
            .map_err(ItemAccessError::Access)?
    }
}

/// Trusted host grant issuer/revoker, not a recipient self-service capability.
#[derive(SystemParam)]
pub struct ItemAdministration<'w> {
    world: WorldId,
    data: Res<'w, ItemRuntimeData>,
    authority: ResMut<'w, ItemRuntimeAuthority>,
}

impl ItemAdministration<'_> {
    /// Issues an independent Read grant for a published item.
    pub fn grant_read(&mut self, item: ItemId) -> Result<ItemReadAccess, ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        self.authority
            .owners
            .get_mut(&item)
            .ok_or(ItemAccessError::UnknownItem)?
            .grant_read()
            .map(ItemReadAccess)
            .map_err(ItemAccessError::Access)
    }

    /// Issues an independent Modify grant without read permission.
    pub fn grant_modify(&mut self, item: ItemId) -> Result<ItemModifyAccess, ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        self.authority
            .owners
            .get_mut(&item)
            .ok_or(ItemAccessError::UnknownItem)?
            .grant_modify()
            .map(ItemModifyAccess)
            .map_err(ItemAccessError::Access)
    }

    /// Revokes this exact item's grant and all of its token clones.
    pub fn revoke_read(
        &mut self,
        item: ItemId,
        access: &ItemReadAccess,
    ) -> Result<bool, ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        self.authority
            .owners
            .get_mut(&item)
            .ok_or(ItemAccessError::UnknownItem)?
            .revoke(&access.0.grant_id())
            .map_err(ItemAccessError::Access)
    }

    /// Revokes this exact item's Modify grant and all of its token clones.
    pub fn revoke_modify(
        &mut self,
        item: ItemId,
        access: &ItemModifyAccess,
    ) -> Result<bool, ItemAccessError> {
        check_world(self.world, &self.data, &self.authority)?;
        self.authority
            .owners
            .get_mut(&item)
            .ok_or(ItemAccessError::UnknownItem)?
            .revoke(&access.0.grant_id())
            .map_err(ItemAccessError::Access)
    }
}
