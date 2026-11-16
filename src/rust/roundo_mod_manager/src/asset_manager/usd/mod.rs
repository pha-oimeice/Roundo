//! Unified Mod-owned USD sources and `asset:author.mod.resource` resolution.
//!
//! Batch import registers every name before any source is composed. Names and
//! ownership remain catalogue identity, not metadata fields. Ordinary paths are
//! layer-relative inside discovered Mod roots; there is no CWD/search fallback.
//! Both path and URI references obey the originating Mod's declared dependency
//! closure, including references authored by a transitive dependency's layers.
//!
//! This category loads source closures, not renderable models or validated
//! skeletal bindings. Upstream discovers files across all variants and payloads;
//! the caller separately supplies the Stage's payload policy. Unsupported clip
//! templates/variable expressions, unreadable dependencies and composition
//! diagnostics fail rather than publishing a partial successful prototype.
//! Named asset dependencies use the manager's existing acyclic prerequisite
//! graph. A cycle there is a manager rejection, not a claim that every cyclic
//! USD file-reference graph is intrinsically invalid USD.

mod error;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use openusd::{
    ar::{ResolvedPath, Resolver, SharedBuffer},
    usd::{PrimPredicate, Stage},
    usd_utils,
};

use crate::source::{
    usd_imports::{ImportDirectory, Location},
    usd_resolver::ImportResolver,
};
use crate::{
    AssetError, AssetKey, AssetLoadContext, AssetPrototype, asset_manager::catalogue::AssetLoader,
};

pub use error::UsdError;
pub use openusd::{
    usd::InitialLoadSet as UsdPayloadPolicy, usd_utils::Dependencies as UsdDependencies,
};

/// Explicit native import input. The name is supplied separately as an AssetName.
/// No TOML declaration schema, default directory or payload policy is inferred.
#[derive(Debug)]
pub struct UsdSource {
    pub(crate) path: PathBuf,
    pub(crate) payloads: UsdPayloadPolicy,
}

impl UsdSource {
    /// Selects a file relative to its owning Mod root and an explicit payload policy.
    /// Batch import checks containment, format and duplicate source identities.
    pub fn new(path: impl Into<PathBuf>, payloads: UsdPayloadPolicy) -> Self {
        Self {
            path: path.into(),
            payloads,
        }
    }
}

/// Validated source metadata sharing one immutable import namespace.
/// Constructed only during ModLoader initialization, not by asset consumers.
/// The import directory is shared by reference; source location has one owner.
pub struct UsdDefinition {
    pub(crate) location: Arc<Location>,
    pub(crate) directory: Arc<ImportDirectory>,
}

impl std::fmt::Debug for UsdDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsdDefinition")
            .field("path", &self.location.path)
            .field("payloads", &self.location.payloads)
            .finish_non_exhaustive()
    }
}

impl UsdDefinition {
    /// Canonical source location; not the business asset name.
    pub fn source_path(&self) -> &Path {
        Path::new(&self.location.path)
    }

    /// Explicit Stage payload policy supplied with this declaration.
    pub fn payload_policy(&self) -> UsdPayloadPolicy {
        self.location.payloads
    }

    fn capture(&self, key: &AssetKey) -> Result<(UsdPrototype, BTreeSet<AssetKey>), UsdError> {
        let resolver = ImportResolver::new(Arc::clone(&self.directory), key.clone());
        let opened = Stage::builder()
            .resolver(resolver.clone())
            .load(self.location.payloads)
            .open(&self.location.path);
        if let Some(error) = resolver.failure() {
            return Err(error);
        }
        let stage = opened?;
        let discovered = usd_utils::all_dependencies(&stage, stage.root_layer().identifier());
        if let Some(error) = resolver.failure() {
            return Err(error);
        }
        let dependencies = discovered?;
        if !dependencies.unresolved.is_empty() {
            return Err(UsdError::Unresolved(dependencies.unresolved));
        }
        // Opening a Stage is insufficient: composition indices are lazy.
        let traversed = stage.traverse(PrimPredicate::DEFAULT_PROXIES, |_| {});
        if let Some(error) = resolver.failure() {
            return Err(error);
        }
        traversed?;
        let errors = stage.composition_errors();
        if !errors.is_empty() {
            return Err(UsdError::Composition(
                errors.iter().map(ToString::to_string).collect(),
            ));
        }
        // Discovery resolves non-layer assets but does not read their bytes.
        // Capture them too; otherwise a readable USD could conceal an unreadable
        // texture or another opaque dependency until a later consumer uses it.
        for identifier in &dependencies.assets {
            resolver
                .open_asset(&ResolvedPath::new(identifier))
                .map_err(|error| UsdError::io(identifier, error))?;
        }
        if let Some(error) = resolver.failure() {
            return Err(error);
        }
        let mut state = resolver.state.lock().expect("USD read state poisoned");
        Ok((
            UsdPrototype {
                dependencies,
                files: std::mem::take(&mut state.files),
            },
            std::mem::take(&mut state.assets),
        ))
    }
}

impl AssetLoader for UsdDefinition {
    fn load(&self, context: &mut AssetLoadContext<'_>) -> Result<Arc<AssetPrototype>, AssetError> {
        let (mut prototype, dependencies) =
            self.capture(context.key())
                .map_err(|error| AssetError::LoadFailed {
                    asset: context.key().clone(),
                    source: Arc::new(error),
                })?;
        for dependency in dependencies {
            let handle = context.require(&dependency)?;
            context.with_dependency(&handle, |payload| match payload {
                AssetPrototype::Usd(child) => prototype.merge(child).map_err(AssetError::from),
                other => Err(AssetError::KindMismatch {
                    asset: dependency.clone(),
                    actual: *other.kind(),
                }),
            })??;
        }
        Ok(Arc::new(AssetPrototype::Usd(prototype)))
    }
}

/// Immutable source snapshot and inventory; not a live or authorable Stage.
///
/// All buffers are manager-owned. The interface lends bytes, not owning source
/// or Stage clones, so ordinary handle clones cannot postpone unloading.
/// Within a load, repeated reads use the captured bytes for consistency. There
/// is no cross-asset/global I/O cache or point-in-time filesystem transaction.
#[derive(Debug)]
pub struct UsdPrototype {
    dependencies: UsdDependencies,
    files: BTreeMap<String, SharedBuffer>,
}

impl UsdPrototype {
    /// Source identifier of the root layer/package, first in the layer inventory.
    pub fn root_identifier(&self) -> &str {
        self.dependencies
            .layers
            .first()
            .expect("upstream discovery includes the root")
    }

    /// Complete discovered source inventory, including dependencies of named assets.
    /// An admitted prototype always has an empty unresolved list. These are source
    /// locations, not AssetKeys or skeletal/animation semantic relationships.
    pub fn dependencies(&self) -> &UsdDependencies {
        &self.dependencies
    }

    /// Borrows a captured layer, package, archive entry or opaque source's bytes.
    /// Unknown identifiers return None; this never performs a late disk read.
    pub fn source_bytes(&self, identifier: &str) -> Option<&[u8]> {
        self.files.get(identifier).map(|bytes| bytes.as_ref())
    }

    /// Identifiers for all captured buffers, including containing package files.
    pub fn source_identifiers(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    fn merge(&mut self, dependency: &Self) -> Result<(), UsdError> {
        for (identifier, bytes) in &dependency.files {
            if let Some(existing) = self.files.get(identifier) {
                if existing.as_ref() != bytes.as_ref() {
                    return Err(UsdError::SourceChanged(identifier.clone()));
                }
            } else {
                self.files.insert(identifier.clone(), bytes.clone());
            }
        }
        let root = self.root_identifier().to_owned();
        let mut layers: BTreeSet<_> = self
            .dependencies
            .layers
            .iter()
            .chain(&dependency.dependencies.layers)
            .cloned()
            .collect();
        layers.remove(&root);
        let assets: BTreeSet<_> = self
            .dependencies
            .assets
            .iter()
            .chain(&dependency.dependencies.assets)
            .cloned()
            .filter(|asset| asset != &root && !layers.contains(asset))
            .collect();
        self.dependencies.layers = std::iter::once(root).chain(layers).collect();
        self.dependencies.assets = assets.into_iter().collect();
        Ok(())
    }
}
