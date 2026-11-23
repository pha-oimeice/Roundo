use std::sync::Arc;

use crate::{
    AssetError, AssetLoadContext, AssetName,
    usd::{UsdDefinition, UsdPrototype},
};

/// Finite asset categories supported by this crate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AssetKind {
    /// USD source and its resolved composition dependencies, without rendering.
    Usd,
}

/// Catalogue identity: category plus the existing Mod-qualified resource name.
///
/// The category scopes the name, as ResourceName itself does not encode a type.
/// This is a declaration key, not a loaded instance identity or access grant.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AssetKey {
    kind: AssetKind,
    name: AssetName,
}

impl AssetKey {
    /// Combines an explicit category with a validated Mod resource name.
    pub fn new(kind: AssetKind, name: AssetName) -> Self {
        Self { kind, name }
    }

    /// Category namespace of this declaration.
    pub fn kind(&self) -> &AssetKind {
        &self.kind
    }

    /// Mod-qualified name, kept outside category metadata.
    pub fn name(&self) -> &AssetName {
        &self.name
    }
}

/// Closed declarations of category-specific source and metadata schemas.
///
/// Variants carry their own source/metadata, not a copied catalogue name or
/// an additional kind tag. A USD declaration is prepared by batch import so
/// cross-Mod references see the complete directory before any loading starts.
#[derive(Debug)]
pub enum AssetDefinition {
    /// A USD source in a validated unified Mod import directory.
    Usd(UsdDefinition),
}

impl AssetDefinition {
    /// Category derived from the declaration variant, never separately stored.
    pub fn kind(&self) -> &AssetKind {
        match self {
            Self::Usd(_) => &AssetKind::Usd,
        }
    }

    pub(crate) fn loader(&self) -> &dyn AssetLoader {
        match self {
            Self::Usd(definition) => definition,
        }
    }
}

/// Closed immutable loaded prototype payloads, without gameplay instance state.
///
/// Category payload schemas belong to this crate. The enum is borrowed through
/// a manager read grant; no owning prototype clone is exposed.
#[derive(Debug)]
pub enum AssetPrototype {
    /// Immutable USD source bytes and the resolved dependency inventory.
    Usd(UsdPrototype),
}

impl AssetPrototype {
    /// Category derived from the payload variant, never separately stored.
    pub fn kind(&self) -> &AssetKind {
        match self {
            Self::Usd(_) => &AssetKind::Usd,
        }
    }
}

/// Implemented only by concrete category declarations within this crate.
/// Dependency meaning stays in the loader; context owns dependency acquisition.
pub(crate) trait AssetLoader: Send + Sync {
    fn load(&self, context: &mut AssetLoadContext<'_>) -> Result<Arc<AssetPrototype>, AssetError>;
}
