//! Stores independent block-related item data. This module does not select an
//! item, dispatch a behavior, grant use authority or modify world voxels.

use bevy::{
    ecs::system::{RunSystemOnce, SystemParam},
    prelude::*,
};
use roundo_block_interaction::{BlockDestructionPermit, BlockItemSchemas, BlockPlacementPermit};
use roundo_inventory::{Inventories, InventoryHandle, ItemId};
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

/// Explicit host composition without automatic input selection or gameplay
/// use authority. The target must already exist. Invalid data
/// and targets reject before issuance. Once issued, an ID is never reclaimed;
/// later failures report it explicitly and must not be automatically retried.
pub(crate) fn create_item(
    world: &mut World,
    issuer: &mut ItemIssuer,
    inventory: InventoryHandle,
    placement: Option<BlockPlacementPermit>,
    destruction: Option<BlockDestructionPermit>,
) -> Result<BlockItemPermissions, String> {
    if placement.as_ref().is_some_and(|data| !data.validate())
        || destruction.as_ref().is_some_and(|data| !data.validate())
    {
        return Err("block item reach must be finite and positive".into());
    }
    world
        .run_system_once_with(
            |In(inventory): In<InventoryHandle>, inventories: Inventories| {
                inventories.owner(inventory)
            },
            inventory,
        )
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let item = issuer.issue().map_err(|error| error.to_string())?;
    let mut assigned = false;
    let result = (|| {
        world
            .run_system_once_with(
                |In((inventory, item)): In<(InventoryHandle, ItemId)>, inventories: Inventories| {
                    inventories
                        .owner(inventory)
                        .map_err(|error| error.to_string())?;
                    if inventories
                        .holder(item)
                        .map_err(|error| error.to_string())?
                        .is_some()
                    {
                        return Err("the issued item identity already has custody".to_string());
                    }
                    Ok(())
                },
                (inventory, item),
            )
            .map_err(|error| error.to_string())??;
        world
            .run_system_once_with(
                |In(item): In<ItemId>, mut publisher: ItemPublisher| {
                    publisher.publish_instance(item)
                },
                item,
            )
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        // Establish the requested custody before initializing data. If a later
        // native operation fails, the issued item stays discoverable here.
        world
            .run_system_once_with(
                |In((inventory, item)): In<(InventoryHandle, ItemId)>,
                 mut inventories: Inventories| {
                    inventories.insert(inventory, item)
                },
                (inventory, item),
            )
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        assigned = true;
        let (read, modify) = world
            .run_system_once_with(
                |In(item): In<ItemId>, mut admin: ItemAdministration| {
                    let read = admin.grant_read(item)?;
                    match admin.grant_modify(item) {
                        Ok(modify) => Ok((read, modify)),
                        Err(error) => {
                            admin.revoke_read(item, &read)?;
                            Err(error)
                        }
                    }
                },
                item,
            )
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let schemas = world.resource::<BlockItemSources>().schemas;
        let initialize = modify.clone();
        let initialized = world.run_system_once_with(
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
        );
        // Attempt revocation even if the initialization system itself failed.
        let revoked = world
            .run_system_once_with(
                move |mut admin: ItemAdministration| admin.revoke_modify(item, &modify),
                (),
            )
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if !revoked {
            return Err("item initialization Modify grant was not revoked".into());
        }
        let initialized = initialized
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()));
        if let Err(error) = initialized {
            world
                .run_system_once_with(
                    move |mut admin: ItemAdministration| admin.revoke_read(item, &read),
                    (),
                )
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            return Err(error);
        }
        world
            .resource_mut::<BlockItemSources>()
            .reads
            .insert(item, read);
        world
            .run_system_once_with(
                |In(item): In<ItemId>, data: BlockItemData| data.permissions(item),
                item,
            )
            .map_err(|error| error.to_string())?
    })();
    match result {
        Ok(permissions) => {
            bevy::log::info!(
                "item initialized: instance={}, inventory={:?}, placement={:?}, destruction={:?}",
                item.as_u128(),
                inventory,
                permissions.placement_reach,
                permissions.destruction_reach
            );
            Ok(permissions)
        }
        Err(error) => {
            let message = format!(
                "物品 {} 初始化失败；ID 不会回收。{}：{error}",
                item.as_u128(),
                if assigned {
                    "物品已分配到所选物品栏，但初始化未全部确认，请检查物品栏，不要自动重试"
                } else {
                    "尚未完成物品栏分配，请检查服务端，不要自动重试"
                }
            );
            bevy::log::error!("{message}");
            Err(message)
        }
    }
}

/// Readback of the stored composition after explicit creation. Custody and data
/// access are validated without interpreting the payload as a gameplay method.
#[derive(SystemParam)]
pub(crate) struct BlockItemData<'w, 's> {
    sources: Res<'w, BlockItemSources>,
    items: ItemReads<'w>,
    inventories: Inventories<'w, 's>,
}

impl BlockItemData<'_, '_> {
    fn permissions(&self, item: ItemId) -> Result<BlockItemPermissions, String> {
        self.inventories
            .holder(item)
            .map_err(|error| error.to_string())?
            .ok_or("item has no live custody")?;
        let read = self
            .sources
            .reads
            .get(&item)
            .ok_or("item read grant is unavailable")?;
        let placement_reach = self
            .items
            .with_specialized(self.sources.schemas.placement, item, read, |value| {
                value.reach
            })
            .map_err(|error| error.to_string())?;
        let destruction_reach = self
            .items
            .with_specialized(self.sources.schemas.destruction, item, read, |value| {
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

/// Owned readback of the actual stored sources, not an echo of an edit request.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockItemPermissions {
    pub item: ItemId,
    pub placement_reach: Option<f32>,
    pub destruction_reach: Option<f32>,
}
