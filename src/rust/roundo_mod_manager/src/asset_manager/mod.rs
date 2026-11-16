//! Explicit lifecycle and access to ModLoader-bound asset prototypes.
//!
//! This module has no Mod discovery, configuration scanning or public source
//! registration interface. Loading reuses an existing prototype and issues a
//! non-owning DSA read grant. Unload ends the prototype and its grants, rejects
//! loaded dependents and does not destroy already materialized game instances.

pub(crate) mod catalogue;
mod error;
mod handle;
mod manager;
pub mod usd;

pub use crate::ResourceName as AssetName;
pub use catalogue::{AssetDefinition, AssetKey, AssetKind, AssetPrototype};
pub use error::AssetError;
pub use handle::AssetHandle;
pub use manager::{AssetLoadContext, AssetManager};
pub use roundo_dsa::data_structure::resource_access::{
    Limits as AssetLimits, ResourceId as AssetInstanceId,
};
