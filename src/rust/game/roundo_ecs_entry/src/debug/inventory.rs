//! Inventory diagnostics adapt native custody only; transport and Player admission
//! remain in the shared diagnostic path. Selectors never establish ownership.
use super::*;
use bevy::ecs::system::RunSystemOnce;
use roundo_inventory::{Inventories, InventoryHandle, ItemId};
use std::collections::{HashMap, HashSet};

#[derive(Resource, Default)]
pub(super) struct InventorySelectors {
    next: u64,
    names: HashMap<InventoryHandle, String>,
}

pub(super) fn valid_item(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 39
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && text.parse::<u128>().is_ok()
}

impl InventorySelectors {
    fn sync(&mut self, inventories: &Inventories, state: &DebugState) -> Result<(), DebugFailure> {
        let handles = inventories.all().map_err(failure)?;
        let mut selected = Vec::new();
        for handle in handles {
            let body = inventories.owner(handle).map_err(failure)?.body();
            if state.bodies.contains_key(&body) && selected.len() < MAX_TARGETS {
                selected.push(handle);
            }
        }
        let keep: HashSet<_> = selected.iter().copied().collect();
        self.names.retain(|handle, _| keep.contains(handle));
        for handle in selected {
            self.name(handle);
        }
        Ok(())
    }

    fn name(&mut self, handle: InventoryHandle) -> String {
        self.names
            .entry(handle)
            .or_insert_with(|| DebugState::allocate(&mut self.next, "inventory"))
            .clone()
    }

    fn resolve(&self, name: &str) -> Result<InventoryHandle, DebugFailure> {
        self.names
            .iter()
            .find_map(|(handle, candidate)| (candidate == name).then_some(*handle))
            .ok_or_else(|| {
                DebugFailure::new(
                    "stale_inventory",
                    "物品栏不存在或诊断选择器已失效，请刷新列表",
                )
            })
    }
}

fn summary(
    handle: InventoryHandle,
    selectors: &InventorySelectors,
    inventories: &Inventories,
    state: &DebugState,
    instances: &LifeInstances,
    roles: &BodyRoles,
    assemblies: Option<&CreatureAssemblyRegistry>,
) -> Result<DebugInventorySummary, DebugFailure> {
    let owner = inventories.owner(handle).map_err(failure)?;
    let body = state.bodies.get(&owner.body()).ok_or_else(|| {
        DebugFailure::new("capacity", "物品栏所属概念体超出诊断索引范围，请刷新列表")
    })?;
    let mut owner_summary = body_summary(body, owner.body(), state, instances, roles, assemblies)?;
    owner_summary.is_public = owner == inventories.public_owner().map_err(failure)?;
    Ok(DebugInventorySummary {
        inventory: selectors.names.get(&handle).cloned().ok_or_else(|| {
            DebugFailure::new("stale_inventory", "物品栏不在当前诊断索引中，请刷新列表")
        })?,
        owner: owner_summary,
        is_public: handle == inventories.public_inventory().map_err(failure)?,
        item_count: inventories
            .item_count(handle)
            .map_err(failure)?
            .try_into()
            .unwrap_or(u32::MAX),
    })
}

/// Re-resolves a current diagnostic selector and reads native custody facts.
/// Used both before identity reservation and after successful item composition.
pub(super) fn selected(
    world: &mut World,
    selector: &str,
) -> Result<(InventoryHandle, DebugInventorySummary), DebugFailure> {
    world
        .run_system_once_with(
            |In(selector): In<String>,
             state: Res<DebugState>,
             mut selectors: ResMut<InventorySelectors>,
             inventories: Inventories,
             instances: LifeInstances,
             roles: BodyRoles,
             assemblies: Option<Res<CreatureAssemblyRegistry>>| {
                selectors.sync(&inventories, &state)?;
                let handle = selectors.resolve(&selector)?;
                let summary = summary(
                    handle,
                    &selectors,
                    &inventories,
                    &state,
                    &instances,
                    &roles,
                    assemblies.as_deref(),
                )?;
                Ok((handle, summary))
            },
            selector.to_owned(),
        )
        .map_err(failure)?
}

pub(super) fn manage(
    mut state: ResMut<DebugState>,
    mut selectors: ResMut<InventorySelectors>,
    mut inventories: Inventories,
    instances: LifeInstances,
    roles: BodyRoles,
    assemblies: Option<Res<CreatureAssemblyRegistry>>,
) {
    if !state.work.iter().any(|work| {
        work.result.is_none() && matches!(work.request.operation, DebugOperation::Inventory(_))
    }) {
        return;
    }
    let synced = selectors.sync(&inventories, &state);
    let mut work = std::mem::take(&mut state.work);
    for item in &mut work {
        let DebugOperation::Inventory(operation) = &item.request.operation else {
            continue;
        };
        if item.result.is_some() {
            continue;
        }
        item.result = Some((|| {
            synced.as_ref().map_err(Clone::clone)?;
            match operation {
                DebugInventoryOperation::List { body, offset } => {
                    let handles = if let Some(body) = body {
                        let owner = inventories.bind_owner(state.body(body)?).map_err(failure)?;
                        inventories.for_body(owner).map_err(failure)?
                    } else {
                        inventories.all().map_err(failure)?
                    };
                    let total = handles.len();
                    let mut indexed: Vec<_> = handles
                        .into_iter()
                        .filter(|handle| selectors.names.contains_key(handle))
                        .collect();
                    // Native snapshots are unordered. Keep page order stable while
                    // these exact routing names remain live, not by display ordinal.
                    indexed.sort_by(|a, b| selectors.names[a].cmp(&selectors.names[b]));
                    let offset = *offset as usize;
                    let entries = indexed
                        .iter()
                        .skip(offset)
                        .take(PAGE)
                        .map(|handle| {
                            summary(
                                *handle,
                                &selectors,
                                &inventories,
                                &state,
                                &instances,
                                &roles,
                                assemblies.as_deref(),
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(DebugData::Inventories {
                        inventories: entries,
                        total: total.try_into().unwrap_or(u32::MAX),
                        next_offset: (offset.saturating_add(PAGE) < indexed.len())
                            .then_some((offset + PAGE) as u32),
                        truncated: total > indexed.len(),
                    })
                }
                DebugInventoryOperation::Inspect { inventory, offset } => {
                    let handle = selectors.resolve(inventory)?;
                    let summary = summary(
                        handle,
                        &selectors,
                        &inventories,
                        &state,
                        &instances,
                        &roles,
                        assemblies.as_deref(),
                    )?;
                    let mut items = inventories.items(handle).map_err(failure)?;
                    items.sort_unstable();
                    let total = items.len();
                    let offset = *offset as usize;
                    Ok(DebugData::Inventory {
                        summary,
                        items: items
                            .into_iter()
                            .skip(offset)
                            .take(PAGE)
                            .map(|item| item.as_u128().to_string())
                            .collect(),
                        total_items: total.try_into().unwrap_or(u32::MAX),
                        next_offset: (offset.saturating_add(PAGE) < total)
                            .then_some((offset + PAGE) as u32),
                    })
                }
                DebugInventoryOperation::Create { body } => {
                    if inventories.len().map_err(failure)? >= MAX_TARGETS {
                        return Err(DebugFailure::new("capacity", "物品栏诊断创建上限已到达"));
                    }
                    let owner = inventories.bind_owner(state.body(body)?).map_err(failure)?;
                    let handle = inventories.create(owner).map_err(failure)?;
                    Ok(DebugData::InventoryCreated {
                        inventory: selectors.name(handle),
                    })
                }
                DebugInventoryOperation::Remove { inventory } => {
                    let handle = selectors.resolve(inventory)?;
                    inventories.remove(handle).map_err(failure)?;
                    selectors.names.remove(&handle);
                    Ok(DebugData::Done)
                }
                DebugInventoryOperation::Transfer {
                    source,
                    target,
                    item,
                } => {
                    let source = selectors.resolve(source)?;
                    let target = selectors.resolve(target)?;
                    let id = item.parse::<u128>().map_err(failure)?;
                    inventories
                        .transfer(source, target, ItemId::from_u128(id))
                        .map_err(failure)?;
                    Ok(DebugData::Done)
                }
                DebugInventoryOperation::Spill { inventory, item } => {
                    let handle = selectors.resolve(inventory)?;
                    let id = item.parse::<u128>().map_err(failure)?;
                    inventories
                        .take(handle, ItemId::from_u128(id))
                        .map_err(failure)?;
                    Ok(DebugData::Done)
                }
            }
        })());
    }
    state.work = work;
}
