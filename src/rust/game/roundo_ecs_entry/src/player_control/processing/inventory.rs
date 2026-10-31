//! Synchronous read at the authenticated Player FIFO boundary. Local inventory
//! handles remain server-side; no Read-only Access or Public custody grants this view.
use super::*;
use bevy::{ecs::system::SystemState, prelude::*};
use roundo_contracts::inventory::*;
use roundo_inventory::Inventories;

pub(super) fn read_inventory(
    world: &mut World,
    connection: ConnectionId,
    request: InventoryRequest,
) -> Result<Vec<BodyInventories>, InventoryReadError> {
    let admission = world.resource::<Admission>();
    let admitted = admission
        .connection(connection)
        .ok_or(InventoryReadError::NotBound)?;
    let binding = admission
        .submit_player(world, request.controller_id, admitted)
        .map_err(|_| InventoryReadError::NotBound)?;
    if roundo_contracts::ControllerBindingId::from(binding) != request.binding {
        return Err(InventoryReadError::NotBound);
    }
    let route = resolve_controller_command_route(world, request.controller_id)
        .ok_or(InventoryReadError::NotBound)?;
    if route.domain != roundo_controller::IntentDomain::movement() {
        return Err(InventoryReadError::NotBound);
    }
    let members = world
        .resource::<crate::authoritative_creature::CreatureAssemblyRegistry>()
        .get(route.life)
        .ok_or(InventoryReadError::NotBound)?;
    let mut state = SystemState::<Inventories>::new(world);
    let inventories = state
        .get_mut(world)
        .map_err(|_| InventoryReadError::Unavailable)?;
    let mut result = Vec::new();
    let mut inventory_count = 0;
    let mut item_count = 0;
    for (role, body) in [
        (InventoryBodyRole::Intent, members.intent),
        (InventoryBodyRole::Execution, members.execution),
        (InventoryBodyRole::Record, members.record),
    ] {
        let owner = inventories
            .bind_owner(body)
            .map_err(|_| InventoryReadError::Unavailable)?;
        let handles = inventories
            .for_body(owner)
            .map_err(|_| InventoryReadError::Unavailable)?;
        inventory_count += handles.len();
        if inventory_count > 128 {
            return Err(InventoryReadError::TooLarge);
        }
        let mut groups = Vec::new();
        for handle in handles {
            let items = inventories
                .items(handle)
                .map_err(|_| InventoryReadError::Unavailable)?;
            item_count += items.len();
            if item_count > 4096 {
                return Err(InventoryReadError::TooLarge);
            }
            let mut ids: Vec<_> = items.into_iter().map(|item| item.as_u128()).collect();
            ids.sort_unstable();
            groups.push(ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>());
        }
        // The primitive snapshots are unordered. Canonicalize this display-only
        // grouping; ordinals are not inventory identities or mutation targets.
        groups.sort();
        result.push(BodyInventories {
            role,
            inventories: groups,
        });
    }
    Ok(result)
}
