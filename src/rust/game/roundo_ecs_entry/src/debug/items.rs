//! Diagnostic item creation, listing and exact-instance destruction. Routing
//! names retain existing resource identities, never a second native allocator.
use super::*;
use bevy::ecs::system::RunSystemOnce;
use roundo_block_interaction::{BlockDestructionPermit, BlockPlacementPermit};
use roundo_inventory::{Inventories, InventoryHandle, ItemId};
use roundo_items::{ItemAccessError, ItemAdministration, ItemInstanceId, ItemIssuer};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub(super) struct ItemSelectors {
    next: u64,
    targets: HashMap<ItemInstanceId, ItemTarget>,
}

struct ItemTarget {
    item: ItemId,
    selector: String,
}

impl ItemSelectors {
    fn sync(&mut self, administration: &ItemAdministration) -> Result<(), DebugFailure> {
        let mut ended = Vec::new();
        for (expected, target) in &self.targets {
            match administration.instance_id(target.item) {
                Ok(current) if current == *expected => {}
                Ok(_) | Err(ItemAccessError::UnknownItem) => ended.push(expected.clone()),
                Err(error) => return Err(failure(error)),
            }
        }
        for instance in ended {
            self.targets.remove(&instance);
        }
        Ok(())
    }

    fn bind(
        &mut self,
        item: ItemId,
        instance: ItemInstanceId,
    ) -> Result<DebugItemSummary, DebugFailure> {
        if !self.targets.contains_key(&instance) && self.targets.len() >= MAX_TARGETS {
            return Err(DebugFailure::new("capacity", "物品实例诊断索引上限已到达"));
        }
        let target = self.targets.entry(instance).or_insert_with(|| ItemTarget {
            item,
            selector: DebugState::allocate(&mut self.next, "item"),
        });
        Ok(DebugItemSummary {
            item: target.item.as_u128().to_string(),
            selector: target.selector.clone(),
        })
    }

    fn resolve(&self, selector: &str) -> Result<(ItemId, ItemInstanceId), DebugFailure> {
        self.targets
            .iter()
            .find_map(|(instance, target)| {
                (target.selector == selector).then(|| (target.item, instance.clone()))
            })
            .ok_or_else(|| {
                DebugFailure::new("stale_item", "物品实例不存在或选择已失效，请刷新物品列表")
            })
    }
}

/// Host-provided reservation for each diagnostic creation, with no fallback IDs.
///
/// The callback must durably reserve a never-reused range before returning its
/// issuer, including across restarts. Reservation failure must return `Err`, not
/// recycle an identity. This capability is not installed by the ECS constructor.
#[derive(Resource)]
pub struct DebugItemIssuer(Box<dyn FnMut() -> Result<ItemIssuer, String> + Send + Sync>);

impl DebugItemIssuer {
    pub fn new(
        reserve: impl FnMut() -> Result<ItemIssuer, String> + Send + Sync + 'static,
    ) -> Self {
        Self(Box::new(reserve))
    }
}

// Exclusive composition keeps the validated target stable across the native
// publication/data/custody steps. No callback is given access to the World.
pub(super) fn manage(world: &mut World) {
    let requests: Vec<_> = world
        .resource::<DebugState>()
        .work
        .iter()
        .enumerate()
        .filter_map(|(index, work)| {
            if work.result.is_some() || work.deferred {
                return None;
            }
            matches!(
                work.request.operation,
                DebugOperation::CreateItem { .. } | DebugOperation::Item(_)
            )
            .then(|| (index, work.request.operation.clone()))
        })
        .collect();
    for (index, operation) in requests {
        let result = match operation {
            DebugOperation::CreateItem {
                inventory,
                placement_reach,
                destruction_reach,
            } => create(world, &inventory, placement_reach, destruction_reach),
            DebugOperation::Item(DebugItemOperation::List { inventory, offset }) => {
                list(world, &inventory, offset)
            }
            DebugOperation::Item(DebugItemOperation::Remove {
                inventory,
                selector,
            }) => remove(world, &inventory, &selector),
            _ => unreachable!("only item operations were selected"),
        };
        world.resource_mut::<DebugState>().work[index].result = Some(result);
    }
}

fn list(world: &mut World, inventory: &str, offset: u32) -> Result<DebugData, DebugFailure> {
    let (handle, summary) = inventory::selected(world, inventory)?;
    world
        .run_system_once_with(
            |In((handle, summary, offset)): In<(InventoryHandle, DebugInventorySummary, u32)>,
             inventories: Inventories,
             administration: ItemAdministration,
             mut selectors: ResMut<ItemSelectors>| {
                selectors.sync(&administration)?;
                let mut items = inventories.items(handle).map_err(failure)?;
                items.sort_unstable();
                let total = items.len();
                let offset = offset as usize;
                let items = items
                    .into_iter()
                    .skip(offset)
                    .take(PAGE)
                    .map(|item| {
                        let instance = administration.instance_id(item).map_err(failure)?;
                        selectors.bind(item, instance)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(DebugData::Items {
                    inventory: summary,
                    items,
                    total_items: total.try_into().unwrap_or(u32::MAX),
                    next_offset: (offset.saturating_add(PAGE) < total)
                        .then_some((offset + PAGE) as u32),
                })
            },
            (handle, summary, offset),
        )
        .map_err(failure)?
}

fn remove(world: &mut World, inventory: &str, selector: &str) -> Result<DebugData, DebugFailure> {
    let (item, expected) = world.resource::<ItemSelectors>().resolve(selector)?;
    let (source, _) = inventory::selected(world, inventory)?;
    world
        .run_system_once_with(
            |In((source, item)): In<(InventoryHandle, ItemId)>, inventories: Inventories| {
                if inventories.holder(item).map_err(failure)? != Some(source) {
                    return Err(DebugFailure::new(
                        "stale_item_custody",
                        "物品已不在所选物品栏，请刷新物品列表",
                    ));
                }
                Ok(())
            },
            (source, item),
        )
        .map_err(failure)??;
    // Use the identity captured by listing, never rebind a stale business number.
    crate::destroy_item(world, item, &expected)
        .map_err(|error| DebugFailure::new("item_removal_failed", error.to_string()))?;
    world
        .resource_mut::<ItemSelectors>()
        .targets
        .remove(&expected);
    Ok(DebugData::ItemRemoved {
        item: item.as_u128().to_string(),
        selector: selector.to_owned(),
    })
}

fn create(
    world: &mut World,
    selector: &str,
    placement: Option<f32>,
    destruction: Option<f32>,
) -> Result<DebugData, DebugFailure> {
    if [placement, destruction]
        .into_iter()
        .flatten()
        .any(|reach| !reach.is_finite() || reach <= 0.0)
    {
        return Err(DebugFailure::new(
            "invalid_item",
            "物品功能距离必须为有限正数",
        ));
    }
    let (target, _) = inventory::selected(world, selector)?;
    let mut issuer = {
        let mut host = world.get_resource_mut::<DebugItemIssuer>().ok_or_else(|| {
            DebugFailure::new("issuance_unavailable", "服务端未安装物品 ID 签发能力")
        })?;
        (host.0)().map_err(|error| DebugFailure::new("identity_reservation_failed", error))?
    };
    let created = crate::block_item::create_item(
        world,
        &mut issuer,
        target,
        placement.map(|reach| BlockPlacementPermit { reach }),
        destruction.map(|reach| BlockDestructionPermit { reach }),
    )
    .map_err(|error| DebugFailure::new("item_creation_failed", error))?;
    let (_, inventory) = inventory::selected(world, selector).map_err(|error| {
        DebugFailure::new(
            "item_readback_failed",
            format!(
                "物品 {} 已创建，但归属回读失败：{}。请刷新物品栏，不要重复创建",
                created.item.as_u128(),
                error.message
            ),
        )
    })?;
    Ok(DebugData::ItemCreated {
        item: created.item.as_u128().to_string(),
        inventory,
        placement_reach: created.placement_reach,
        destruction_reach: created.destruction_reach,
    })
}
