//! Passive, independently composable block-related item data.
//! These payloads do not grant use authority, dispatch a method or edit voxels.
use roundo_items::{ItemDataBuilder, ItemDataError, ItemSchema};

/// Stored placement-related reach; not an executable method or use grant.
#[derive(Clone, Copy, Debug)]
pub struct BlockPlacementPermit {
    pub reach: f32,
}

/// Stored removal-related reach; not an executable method or use grant.
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
