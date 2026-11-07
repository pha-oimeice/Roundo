//! Diagnostic item composition uses the host's durable identity reservation,
//! existing custody selectors and the same native sources as the startup item.
use super::*;
use roundo_block_interaction::{BlockDestructionPermit, BlockPlacementPermit};
use roundo_items::ItemIssuer;

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
            let DebugOperation::CreateItem {
                inventory,
                placement_reach,
                destruction_reach,
            } = &work.request.operation
            else {
                return None;
            };
            Some((
                index,
                inventory.clone(),
                *placement_reach,
                *destruction_reach,
            ))
        })
        .collect();
    for (index, inventory, placement, destruction) in requests {
        let result = create(world, &inventory, placement, destruction);
        world.resource_mut::<DebugState>().work[index].result = Some(result);
    }
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
