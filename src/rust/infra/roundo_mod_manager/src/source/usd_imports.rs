use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use super::SourceDirectory;
use crate::{AssetDefinition, AssetError, AssetKey, AssetKind, AssetName};
use crate::{
    ModId, ResourceReferenceError,
    usd::{UsdDefinition, UsdError, UsdSource},
};
use openusd::sdf;

#[derive(Debug)]
pub(crate) struct Location {
    pub path: String,
    pub payloads: openusd::usd::InitialLoadSet,
}

pub(crate) struct ImportDirectory {
    directory: Arc<SourceDirectory>,
    sources: BTreeMap<AssetKey, Arc<Location>>,
    pub(crate) keys_by_path: BTreeMap<String, AssetKey>,
    local_names: BTreeMap<ModId, BTreeSet<String>>,
}

impl ImportDirectory {
    pub fn prepare(
        directory: Arc<SourceDirectory>,
        sources: impl IntoIterator<Item = (AssetName, UsdSource)>,
    ) -> Result<Vec<(AssetKey, Arc<AssetDefinition>)>, AssetError> {
        let mut directory = Self {
            directory,
            sources: BTreeMap::new(),
            keys_by_path: BTreeMap::new(),
            local_names: BTreeMap::new(),
        };
        for (name, source) in sources {
            let key = AssetKey::new(AssetKind::Usd, name);
            if directory.sources.contains_key(&key) {
                return Err(AssetError::AlreadyDeclared(key));
            }
            let bound = directory
                .directory
                .bind(key.name().owner(), &source.path)
                .map_err(UsdError::Import)?;
            let path = bound
                .path()
                .to_str()
                .ok_or_else(|| UsdError::Import("USD paths must be UTF-8".into()))?
                .to_owned();
            let extension = Path::new(&path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("");
            if sdf::LayerRegistry::find_by_extension(extension).is_none() {
                return Err(UsdError::Import(format!(
                    "not a supported USD layer/package source: {path}"
                ))
                .into());
            }
            if let Some(existing) = directory.keys_by_path.get(&path) {
                return Err(UsdError::Import(format!(
                    "source {path} is declared twice: {existing:?} and {key:?}"
                ))
                .into());
            }
            directory
                .local_names
                .entry(key.name().owner().clone())
                .or_default()
                .insert(key.name().local().to_owned());
            directory.keys_by_path.insert(path.clone(), key.clone());
            directory.sources.insert(
                key,
                Arc::new(Location {
                    path,
                    payloads: source.payloads,
                }),
            );
        }
        let directory = Arc::new(directory);
        Ok(directory
            .sources
            .iter()
            .map(|(key, location)| {
                (
                    key.clone(),
                    Arc::new(AssetDefinition::Usd(UsdDefinition {
                        location: Arc::clone(location),
                        directory: Arc::clone(&directory),
                    })),
                )
            })
            .collect())
    }

    pub(crate) fn owner(&self, identifier: &str) -> Result<&ModId, UsdError> {
        let outer = openusd::ar::split_package_relative_path_outer(identifier)
            .map(|(outer, _)| outer)
            .unwrap_or_else(|| identifier.to_owned());
        self.directory
            .roots
            .iter()
            .find(|(_, root)| Path::new(&outer).starts_with(root))
            .map(|(owner, _)| owner)
            .ok_or_else(|| {
                UsdError::Access(format!(
                    "source is outside imported Mod roots: {identifier}"
                ))
            })
    }

    pub(crate) fn authorize(&self, origin: &ModId, target: &ModId) -> Result<(), UsdError> {
        if origin == target {
            return Ok(());
        }
        let closure = self
            .directory
            .mods
            .dependency_closure(origin)
            .map_err(|error| UsdError::Access(error.to_string()))?;
        if closure.contains(target) {
            Ok(())
        } else {
            Err(UsdError::Access(format!(
                "{origin} has no declared dependency on {target}"
            )))
        }
    }

    pub(crate) fn asset(&self, origin: &ModId, uri: &str) -> Result<&Location, UsdError> {
        let reference = uri
            .strip_prefix("asset:")
            .ok_or_else(|| UsdError::Reference(uri.into()))?;
        // asset: requires a complete ResourceName. Same-owner complete names
        // normalize to the existing local-reference branch, not a second policy.
        if reference.split('.').count() != 3 {
            return Err(UsdError::Reference(format!(
                "expected asset:author.mod.resource, got {uri}"
            )));
        }
        let local = self.local_names.get(origin).cloned().unwrap_or_default();
        let owner_prefix = format!("{origin}.");
        let reference = reference.strip_prefix(&owner_prefix).unwrap_or(reference);
        let name = self
            .directory
            .mods
            .resolve_resource_reference(origin, &local, reference)
            .map_err(|error| match &error {
                ResourceReferenceError::UndeclaredDependency { .. }
                | ResourceReferenceError::ModLoader(_) => UsdError::Access(error.to_string()),
                _ => UsdError::Reference(error.to_string()),
            })?;
        let key = AssetKey::new(AssetKind::Usd, name);
        self.sources.get(&key).map(Arc::as_ref).ok_or_else(|| {
            UsdError::Reference(format!("asset is not in the unified USD import: {uri}"))
        })
    }
}
