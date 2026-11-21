//! Serialized host coordination of item lifetime termination.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use roundo_inventory::ItemId;
use roundo_items::{ItemDestructionError, ItemInstanceId, ItemLifecycle};

use crate::world_items::WorldItemError;

/// Recoverable rejection leaves item data, custody and host representations
/// unchanged. Panic/abort/OOM and arbitrary payload/component destructor side
/// effects are outside this guarantee.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemDeletionError {
    Item(ItemDestructionError),
    Representation(WorldItemError),
    /// The required native system parameters could not be obtained.
    Execution(String),
}

impl std::fmt::Display for ItemDeletionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Item(error) => write!(formatter, "{error}"),
            Self::Representation(error) => write!(formatter, "{error}"),
            Self::Execution(error) => write!(formatter, "item destruction unavailable: {error}"),
        }
    }
}

impl std::error::Error for ItemDeletionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Item(error) => Some(error),
            Self::Representation(error) => Some(error),
            Self::Execution(_) => None,
        }
    }
}

/// Ends one exact item instance, including an item owned by Public.
///
/// This is a trusted host lifecycle interface, not a Player request or a data
/// access grant. Capture `expected` from the existing item administration/grant
/// when selecting the instance; never resolve a stale business number afresh to
/// manufacture the expected identity immediately before deletion.
///
/// Exclusive World access prevents interleaved creation or transfer. Native
/// validation precedes all writes. Success removes custody, all native payloads,
/// the authorization Owner and this host's cached read grant, and synchronously
/// despawns any registered spatial representation. It does not delete an owner
/// Concept Body or inventory, transfer to Public, or defer work to another tick.
///
/// Repeated deletion, stale/wrong-domain identities, absent custody and malformed
/// registered representations return errors. No ID blacklist is created: a new
/// publication may use the business number again but cannot revive the old
/// instance's identity or grants. Host issuance/recycling policy is unchanged.
pub fn destroy_item(
    world: &mut World,
    item: ItemId,
    expected: &ItemInstanceId,
) -> Result<(), ItemDeletionError> {
    let representation = crate::world_items::prepare_destruction(world, item)
        .map_err(ItemDeletionError::Representation)?;
    world
        .run_system_once_with(
            |In((item, expected)): In<(ItemId, ItemInstanceId)>, mut lifecycle: ItemLifecycle| {
                lifecycle.destroy(item, &expected)
            },
            (item, expected.clone()),
        )
        .map_err(|error| ItemDeletionError::Execution(error.to_string()))?
        .map_err(ItemDeletionError::Item)?;
    crate::block_item::forget_destroyed_item(world, item);
    crate::world_items::finish_destruction(world, item, representation);
    Ok(())
}
