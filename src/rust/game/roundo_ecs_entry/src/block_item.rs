//! Composition for the one block-use item. Custody, data access and method
//! permission remain distinct: an instance ID or a default reference is no grant.

use bevy::{
    ecs::system::{RunSystemOnce, SystemParam},
    prelude::*,
};
use roundo_block_interaction::{BlockDestructionPermit, BlockItemSchemas, BlockPlacementPermit};
use roundo_contracts::{ItemInstanceId, ItemUseMethod};
use roundo_inventory::{Inventories, ItemId};
use roundo_items::{
    ItemAdministration, ItemData, ItemDataBuilder, ItemDefinitions, ItemIssuer, ItemPublisher,
    ItemReadAccess, ItemReads, ItemRuntimeSetup, ItemWrites,
};
use std::collections::HashMap;

#[derive(Resource)]
struct BlockItemSources {
    schemas: BlockItemSchemas,
    reads: HashMap<ItemId, ItemReadAccess>,
}

/// Default input reference for this slice, not an equipment state or a permit.
#[derive(Resource)]
pub(crate) struct BlockItemReference(pub(crate) ItemId);

pub(crate) fn install(app: &mut App) {
    let mut builder = ItemDataBuilder::try_new().expect("item data catalogue allocation");
    let schemas = BlockItemSchemas::register(&mut builder).expect("block item source registration");
    let setup = ItemRuntimeSetup::try_new(ItemDefinitions::empty(), builder)
        .expect("item runtime catalogue preparation");
    roundo_items::install_items(app, setup).expect("one item runtime per server App");
    app.insert_resource(BlockItemSources {
        schemas,
        reads: HashMap::new(),
    });
}

/// Creates the slice's single composed instance from host-issued identity and
/// caller-supplied data. No prototype, callback or per-ID behavior is installed.
/// Identity issuance is irreversible. The reference is published only after data
/// and Public custody succeed; initialization errors must abort host startup.
pub fn create_block_item(
    world: &mut World,
    issuer: &mut ItemIssuer,
    placement: Option<BlockPlacementPermit>,
    destruction: Option<BlockDestructionPermit>,
) -> Result<ItemId, String> {
    if world.contains_resource::<BlockItemReference>() {
        return Err("the block item instance is already initialized".into());
    }
    if placement.as_ref().is_some_and(|data| !data.validate())
        || destruction.as_ref().is_some_and(|data| !data.validate())
    {
        return Err("block item reach must be finite and positive".into());
    }
    let item = issuer.issue().map_err(|error| error.to_string())?;
    world
        .run_system_once_with(
            |In(item): In<ItemId>, mut publisher: ItemPublisher| publisher.publish_instance(item),
            item,
        )
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let (read, modify) = world
        .run_system_once_with(
            |In(item): In<ItemId>, mut admin: ItemAdministration| {
                Ok::<_, roundo_items::ItemAccessError>((
                    admin.grant_read(item)?,
                    admin.grant_modify(item)?,
                ))
            },
            item,
        )
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let schemas = world.resource::<BlockItemSources>().schemas;
    let initialize = modify.clone();
    let initialized = world
        .run_system_once_with(
            move |mut writes: ItemWrites| {
                if let Some(value) = placement {
                    writes.set_specialized(schemas.placement, item, &initialize, value)?;
                }
                if let Some(value) = destruction {
                    writes.set_specialized(schemas.destruction, item, &initialize, value)?;
                }
                Ok::<_, roundo_items::ItemAccessError>(())
            },
            (),
        )
        .map_err(|error| error.to_string())?;
    world
        .run_system_once_with(
            move |mut admin: ItemAdministration| admin.revoke_modify(item, &modify),
            (),
        )
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    initialized.map_err(|error| error.to_string())?;
    world
        .run_system_once_with(
            |In(item): In<ItemId>, mut inventories: Inventories| {
                let public = inventories.public_inventory()?;
                inventories.insert(public, item)
            },
            item,
        )
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    world
        .resource_mut::<BlockItemSources>()
        .reads
        .insert(item, read);
    world.insert_resource(BlockItemReference(item));
    bevy::log::info!(
        "block item initialized: instance={}, placement={}, destruction={}",
        item.as_u128(),
        placement.is_some(),
        destruction.is_some()
    );
    Ok(item)
}

/// Execution-side current data access, independent of the custodian's Body.
/// These borrows keep custody and permission stable through the synchronous edit.
#[derive(SystemParam)]
pub(crate) struct BlockItemData<'w, 's> {
    sources: Res<'w, BlockItemSources>,
    items: ItemReads<'w>,
    inventories: Inventories<'w, 's>,
}

impl BlockItemData<'_, '_> {
    pub(crate) fn reach(&self, item: ItemId, method: ItemUseMethod) -> Option<f32> {
        // Any live inventory is sufficient; owner identity is not use authority.
        self.inventories.holder(item).ok().flatten()?;
        let access = self.sources.reads.get(&item)?;
        match method {
            ItemUseMethod::PlaceBlock => {
                self.items
                    .with_specialized(self.sources.schemas.placement, item, access, |value| {
                        value.reach
                    })
            }
            ItemUseMethod::DestroyBlock => self.items.with_specialized(
                self.sources.schemas.destruction,
                item,
                access,
                |value| value.reach,
            ),
        }
        .ok()
        .flatten()
    }
}

/// Owned readback of the actual stored sources, not an echo of an edit request.
#[derive(Clone, Copy, Debug)]
pub struct BlockItemPermissions {
    pub item: ItemId,
    pub placement_reach: Option<f32>,
    pub destruction_reach: Option<f32>,
}

/// Trusted host editing of the slice's data sources. No method is executed and
/// no use authority is granted; a missing reach removes only that source.
#[derive(SystemParam)]
pub struct BlockItemEdits<'w, 's> {
    reference: Option<Res<'w, BlockItemReference>>,
    sources: Res<'w, BlockItemSources>,
    params: ParamSet<'w, 's, (ItemAdministration<'w>, ItemWrites<'w>)>,
}

impl BlockItemEdits<'_, '_> {
    pub fn set_permission(
        &mut self,
        method: ItemUseMethod,
        reach: Option<f32>,
    ) -> Result<BlockItemPermissions, String> {
        if reach.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err("reach must be finite and positive".into());
        }
        let item = self
            .reference
            .as_ref()
            .ok_or("block item is not initialized")?
            .0;
        let modify = self
            .params
            .p0()
            .grant_modify(item)
            .map_err(|error| error.to_string())?;
        let result = {
            let mut writes = self.params.p1();
            match (method, reach) {
                (ItemUseMethod::PlaceBlock, Some(reach)) => writes.set_specialized(
                    self.sources.schemas.placement,
                    item,
                    &modify,
                    BlockPlacementPermit { reach },
                ),
                (ItemUseMethod::DestroyBlock, Some(reach)) => writes.set_specialized(
                    self.sources.schemas.destruction,
                    item,
                    &modify,
                    BlockDestructionPermit { reach },
                ),
                (ItemUseMethod::PlaceBlock, None) => writes
                    .remove_specialized(self.sources.schemas.placement, item, &modify)
                    .map(|_| ()),
                (ItemUseMethod::DestroyBlock, None) => writes
                    .remove_specialized(self.sources.schemas.destruction, item, &modify)
                    .map(|_| ()),
            }
        };
        let revocation = self.params.p0().revoke_modify(item, &modify);
        result.map_err(|error| error.to_string())?;
        revocation.map_err(|error| error.to_string())?;
        bevy::log::info!(
            "block item data changed: instance={}, method={:?}, reach={:?}",
            item.as_u128(),
            method,
            reach
        );
        let access = self
            .sources
            .reads
            .get(&item)
            .ok_or("item read grant is unavailable")?;
        let writes = self.params.p1();
        let placement_reach = writes
            .with_specialized(self.sources.schemas.placement, item, access, |value| {
                value.reach
            })
            .map_err(|error| error.to_string())?;
        let destruction_reach = writes
            .with_specialized(self.sources.schemas.destruction, item, access, |value| {
                value.reach
            })
            .map_err(|error| error.to_string())?;
        Ok(BlockItemPermissions {
            item,
            placement_reach,
            destruction_reach,
        })
    }
}

pub(crate) fn reference(reference: Option<&BlockItemReference>) -> Option<ItemInstanceId> {
    reference.map(|value| ItemInstanceId(value.0.as_u128()))
}
