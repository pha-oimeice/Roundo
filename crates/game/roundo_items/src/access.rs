//! Errors and the private, immutable item authorization target.

use crate::{ItemDataError, ItemInstanceId};
use roundo_infrastructure::dsa::data_structure::resource_access::{
    AccessError, ModifyAccess, ReadAccess, Shared,
};
use roundo_inventory::{InventoryError, ItemId};
use roundo_infrastructure::mod_manager::ResourceName;
use std::{error::Error, fmt};

/// Installation refuses to replace either half of an existing runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemInstallError {
    AlreadyInstalled,
}

impl fmt::Display for ItemInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("item runtime is already installed")
    }
}

impl Error for ItemInstallError {}

/// Rejection of item publication or runtime data access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemAccessError {
    WrongWorld,
    UnknownItem,
    AlreadyPublished,
    UnknownDefinition(ResourceName),
    MissingData,
    Access(AccessError),
    Data(ItemDataError),
}

impl fmt::Display for ItemAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongWorld => formatter.write_str("item runtime belongs to another World"),
            Self::UnknownItem => formatter.write_str("unknown published item"),
            Self::AlreadyPublished => formatter.write_str("item is already published"),
            Self::UnknownDefinition(name) => write!(formatter, "unknown item definition: {name}"),
            Self::MissingData => formatter.write_str("published item has no native data"),
            Self::Access(error) => write!(formatter, "item authorization failed: {error}"),
            Self::Data(error) => write!(formatter, "item data operation failed: {error}"),
        }
    }
}

impl Error for ItemAccessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Data(error) => Some(error),
            _ => None,
        }
    }
}

/// Recoverable rejection of a trusted data/custody destruction commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemDestructionError {
    Item(ItemAccessError),
    Inventory(InventoryError),
}

impl fmt::Display for ItemDestructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item(error) => write!(formatter, "item destruction rejected: {error}"),
            Self::Inventory(error) => write!(formatter, "item custody rejected: {error}"),
        }
    }
}

impl Error for ItemDestructionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Item(error) => Some(error),
            Self::Inventory(error) => Some(error),
        }
    }
}

impl From<ItemAccessError> for ItemDestructionError {
    fn from(error: ItemAccessError) -> Self {
        Self::Item(error)
    }
}

impl From<InventoryError> for ItemDestructionError {
    fn from(error: InventoryError) -> Self {
        Self::Inventory(error)
    }
}

/// Read grant for one entire exact item target, not issuing authority.
/// Clones redistribute the same grant and share its revocation.
///
/// Redistribute the wrapper, not its private authorization representation:
///
///
///
/// Only a trusted [`crate::ItemAdministration`] system can issue another grant:
///
///
#[derive(Clone, Debug)]
pub struct ItemReadAccess(pub(crate) ReadAccess<ItemAccessTarget, Shared>);

/// Modification qualification for one exact item, without read permission.
/// Clones redistribute the same grant and share its revocation.
#[derive(Clone, Debug)]
pub struct ItemModifyAccess(pub(crate) ModifyAccess<ItemAccessTarget, Shared>);

impl ItemReadAccess {
    /// Exact resource identity, not a destruction capability or a business ID.
    /// Retaining it keeps no payload alive and cannot identify a later instance
    /// published with the same ItemId.
    pub fn instance_id(&self) -> &ItemInstanceId {
        self.0.resource_id()
    }
}

impl ItemModifyAccess {
    /// Exact target identity only; modification does not confer destruction rights.
    pub fn instance_id(&self) -> &ItemInstanceId {
        self.0.resource_id()
    }
}

// No payload, shared handle or mutable state is stored in the RA directory.
pub(crate) struct ItemAccessTarget {
    pub(crate) item: ItemId,
}
