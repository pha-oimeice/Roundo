//! Mod initialization and immutable prototype management.
//!
//! ModLoader is the sole initialization authority: it discovers and validates
//! Mods, parses configuration, delivers domain prototypes and binds asset sources.
//! AssetManager never discovers Mods or adds source namespaces after startup;
//! consumers explicitly request load/read/unload through the initialized manager.
//! Registered data are prototypes. UI entities, items and other gameplay instances
//! remain owned by their respective runtime modules.

#![forbid(unsafe_code)]

pub mod asset_manager;
mod mod_loader;
mod mods;
mod source;
pub mod ui;

pub use asset_manager::usd;
pub use asset_manager::{
    AssetDefinition, AssetError, AssetHandle, AssetInstanceId, AssetKey, AssetKind, AssetLimits,
    AssetLoadContext, AssetManager, AssetName, AssetPrototype,
};
pub use mod_loader::{
    ATOMIC_VOXEL_PROTOTYPE_TYPE, INPUT_PROTOTYPE_TYPE, ITEM_PROTOTYPE_TYPE, ModConfig, ModLoader,
    ModManager, ModTypeInput, PrototypeLoadError, PrototypeLoadErrors, PrototypeTypeId,
    UI_PROTOTYPE_TYPE,
};
pub use mods::{
    LoadError, LoadedMod, LoadedMods, ModId, ResourceCandidate, ResourceName,
    ResourceReferenceError, parse_mod_id, resolve_candidates, valid_resource_local_name,
};

use std::sync::{Arc, Mutex};

/// Marker for immutable configuration/prototype registries delivered to domains.
/// Implementations must not represent issued items, UI entities, players or other
/// live gameplay instances. Instantiation belongs to the receiving domain.
pub trait ModPrototype: std::any::Any + Send + Sync {}

/// Shared access to the one initialized AssetManager, never a second manager.
/// Creation/binding is private to ModLoader; clones grant no directory mutation.
#[derive(Clone)]
pub struct AssetAccess(Arc<Mutex<AssetManager>>);

impl AssetAccess {
    pub(crate) fn new(manager: AssetManager) -> Self {
        Self(Arc::new(Mutex::new(manager)))
    }

    /// Executes an explicit asset operation. The callback must not re-enter the
    /// same access endpoint; neither its manager borrow nor payload borrows escape.
    pub fn with<R>(&self, operation: impl FnOnce(&mut AssetManager) -> R) -> R {
        operation(&mut self.0.lock().expect("asset manager lock poisoned"))
    }
}
