//! Passive, independently composable block-use permissions and the use intent.
//! No item identity, schema or data value executes a behavior.

use roundo_contracts::ItemUseMethod;
use roundo_inventory::ItemId;
use roundo_items::{ItemDataBuilder, ItemDataError, ItemSchema};

/// Presence permits the code-implemented placement method at this world-space reach.
#[derive(Clone, Copy, Debug)]
pub struct BlockPlacementPermit {
    pub reach: f32,
}

/// Presence permits the code-implemented destruction method at this world-space reach.
#[derive(Clone, Copy, Debug)]
pub struct BlockDestructionPermit {
    pub reach: f32,
}

roundo_items::declare_specialized_data!(
    BlockPlacementPermit,
    "roundo.block-placement.permit.v1",
    |value: &BlockPlacementPermit| value.reach.is_finite() && value.reach > 0.0
);
roundo_items::declare_specialized_data!(
    BlockDestructionPermit,
    "roundo.block-destruction.permit.v1",
    |value: &BlockDestructionPermit| value.reach.is_finite() && value.reach > 0.0
);

/// Catalogue-bound source identities, not item identities or gameplay authorization.
#[derive(Clone, Copy, Debug)]
pub struct BlockItemSchemas {
    pub placement: ItemSchema<BlockPlacementPermit>,
    pub destruction: ItemSchema<BlockDestructionPermit>,
}

impl BlockItemSchemas {
    pub fn register(builder: &mut ItemDataBuilder) -> Result<Self, ItemDataError> {
        Ok(Self {
            placement: builder.register()?,
            destruction: builder.register()?,
        })
    }
}

/// An intent for Execution to interpret against current item data, not an edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ItemUseCommandEvent {
    pub item: ItemId,
    pub method: ItemUseMethod,
}

roundo_life::declare_event_data!(ItemUseCommandEvent, "roundo.item-use.command.v1", |_| {
    std::mem::size_of::<ItemUseCommandEvent>()
});
