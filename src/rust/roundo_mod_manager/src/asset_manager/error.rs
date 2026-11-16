use std::{error::Error, sync::Arc};

use roundo_dsa::data_structure::resource_access::AccessError;
use thiserror::Error;

use crate::{AssetKey, AssetKind};

/// Rejection of catalogue, loading, dependency or authorized access operations.
#[derive(Clone, Debug, Error)]
pub enum AssetError {
    /// A declaration with this exact category and name already exists.
    #[error("asset already declared: {0:?}")]
    AlreadyDeclared(AssetKey),
    /// No declaration exists for the requested category and name.
    #[error("unknown asset: {0:?}")]
    UnknownAsset(AssetKey),
    /// The declaration exists, but no loaded prototype exists.
    #[error("asset is not loaded: {0:?}")]
    NotLoaded(AssetKey),
    /// A declaration or loader returned a different category than its key.
    #[error("asset category mismatch: {asset:?}, actual={actual:?}")]
    KindMismatch {
        /// Catalogue key that established the expected category.
        asset: AssetKey,
        /// Category actually supplied by the declaration or payload.
        actual: AssetKind,
    },
    /// Loaded consumers must be unloaded first; no graph node was removed.
    #[error("asset has loaded dependents: {asset:?}: {dependents:?}")]
    InUse {
        /// Asset whose unload was rejected.
        asset: AssetKey,
        /// Direct loaded dependents, in catalogue-key order.
        dependents: Vec<AssetKey>,
    },
    /// The chain closes by repeating the first key; no partial root is published.
    #[error("cyclic asset dependency: {0:?}")]
    DependencyCycle(Vec<AssetKey>),
    /// A loader tried to read a handle without acquiring it through its context.
    #[error("asset was not acquired by this loader: {0:?}")]
    UndeclaredDependency(AssetKey),
    /// Category-owned parsing, source or semantic validation failed.
    #[error("cannot load asset {asset:?}: {source}")]
    LoadFailed {
        /// Asset being interpreted by its category loader.
        asset: AssetKey,
        /// Original category/source error; no simulated successful payload exists.
        #[source]
        source: Arc<dyn Error + Send + Sync>,
    },
    /// USD import, source, protocol or completeness rejection.
    #[error(transparent)]
    Usd(#[from] crate::usd::UsdError),
    /// DSA identity, grant, resource-capacity or protected-access rejection.
    #[error(transparent)]
    Access(#[from] AccessError),
}
