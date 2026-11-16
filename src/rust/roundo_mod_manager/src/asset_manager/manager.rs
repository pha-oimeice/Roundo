use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use roundo_dsa::data_structure::resource_access::{
    AccessError, Owner, ResourceAccessManager, Shared,
};

use crate::{
    AssetDefinition, AssetError, AssetHandle, AssetInstanceId, AssetKey, AssetLimits,
    AssetPrototype,
};

struct LoadedAsset {
    owner: Owner<Arc<AssetPrototype>, Shared>,
    // The sole authoritative graph edges. Reverse edges are queried, not copied.
    dependencies: BTreeSet<AssetKey>,
}

#[derive(Default)]
struct LoadTransaction {
    stack: Vec<AssetKey>,
    created: Vec<AssetKey>,
}

/// Catalogue and sole lifecycle owner of immutable asset prototypes.
///
/// Catalogue keys identify declarations; DSA IDs identify individual loads;
/// handles grant reads. None is interchangeable with either of the others.
/// All mutations require exclusive manager access. No global static, Bevy App,
/// graphics infrastructure, background task or implicit source is installed.
/// Drop unloads dependents before dependencies and invalidates every handle.
///
/// Forward dependency edges are stored once. Reverse queries scan loaded edges;
/// they do not maintain a second dependency graph or a reference-count policy.
pub struct AssetManager {
    catalogue: BTreeMap<AssetKey, Arc<AssetDefinition>>,
    resources: ResourceAccessManager<Shared>,
    loaded: BTreeMap<AssetKey, LoadedAsset>,
    keys_by_instance: HashMap<AssetInstanceId, AssetKey>,
}

impl AssetManager {
    /// Creates an empty manager with explicitly supplied DSA storage/grant limits.
    /// No assets, categories, sources, runtime defaults or loading policy are added.
    pub(crate) fn new(
        limits: AssetLimits,
        declarations: impl IntoIterator<Item = (AssetKey, Arc<AssetDefinition>)>,
    ) -> Self {
        Self {
            catalogue: declarations.into_iter().collect(),
            resources: ResourceAccessManager::with_shared_limits(limits),
            loaded: BTreeMap::new(),
            keys_by_instance: HashMap::new(),
        }
    }

    /// Borrows a declaration without loading its prototype or issuing a grant.
    pub fn definition(&self, key: &AssetKey) -> Option<&AssetDefinition> {
        self.catalogue.get(key).map(Arc::as_ref)
    }

    /// Whether this exact catalogue key currently has a loaded prototype.
    /// False does not distinguish an unknown key from an unloaded declaration.
    pub fn is_loaded(&self, key: &AssetKey) -> bool {
        self.loaded.contains_key(key)
    }

    /// Loads the prototype and loader-discovered dependencies synchronously.
    ///
    /// Repeated loads reuse the same prototype/instance and issue independent
    /// read grants. Any returned error removes only nodes created by this call,
    /// in reverse loading order; existing prototypes and grants remain intact.
    /// Unloaded dependency declarations are retained. No load count is acquired.
    pub fn load(&mut self, key: &AssetKey) -> Result<AssetHandle, AssetError> {
        let mut transaction = LoadTransaction::default();
        let result = self
            .ensure_loaded(key, &mut transaction)
            .and_then(|()| self.handle(key));
        if result.is_err() {
            for created in transaction.created.iter().rev() {
                self.remove_loaded(created);
            }
        }
        result
    }

    /// Issues an independent read grant without initiating a load.
    pub fn handle(&mut self, key: &AssetKey) -> Result<AssetHandle, AssetError> {
        if !self.loaded.contains_key(key) {
            return Err(self.missing_loaded(key));
        }
        let loaded = self.loaded.get_mut(key).expect("checked loaded key");
        Ok(AssetHandle {
            access: loaded.owner.grant_read()?,
        })
    }

    /// Reads a prototype only inside a matching live grant's protected callback.
    /// Borrowed payloads cannot escape. Foreign, revoked and unloaded handles
    /// reject; resolving the old name again cannot retarget a stale handle.
    pub fn read<R>(
        &self,
        handle: &AssetHandle,
        read: impl for<'prototype> FnOnce(&'prototype AssetPrototype) -> R,
    ) -> Result<R, AssetError> {
        let key = self.key_for_handle(handle)?;
        let loaded = self.loaded.get(key).expect("instance index is live");
        loaded
            .owner
            .protect(|scope| scope.with_read(&handle.access, |prototype| read(prototype.as_ref())))?
            .map_err(Into::into)
    }

    /// Revokes one grant and all its clones, not independent grants or the asset.
    /// Returns false for an already revoked grant of a still-loaded prototype.
    pub fn revoke(&mut self, handle: &AssetHandle) -> Result<bool, AssetError> {
        let Some(key) = self.keys_by_instance.get(handle.instance_id()) else {
            handle.check()?;
            return Err(AccessError::ResourceMismatch.into());
        };
        self.loaded
            .get_mut(key)
            .expect("instance index is live")
            .owner
            .revoke(&handle.access.grant_id())
            .map_err(Into::into)
    }

    /// Unloads exactly one asset and invalidates all its outstanding handles.
    ///
    /// Loaded dependents cause an error before any mutation. Dependencies stay
    /// loaded until explicitly unloaded; the manager does not choose GC, cascade
    /// or eviction timing. The declaration remains available for a later load.
    pub fn unload(&mut self, key: &AssetKey) -> Result<(), AssetError> {
        if !self.loaded.contains_key(key) {
            return Err(self.missing_loaded(key));
        }
        let dependents = self.dependents(key)?;
        if !dependents.is_empty() {
            return Err(AssetError::InUse {
                asset: key.clone(),
                dependents,
            });
        }
        self.remove_loaded(key);
        Ok(())
    }

    /// Direct dependencies acquired by this prototype's successful loader call.
    pub fn dependencies(&self, key: &AssetKey) -> Result<&BTreeSet<AssetKey>, AssetError> {
        self.loaded
            .get(key)
            .map(|asset| &asset.dependencies)
            .ok_or_else(|| self.missing_loaded(key))
    }

    /// Direct loaded dependents in catalogue-key order, derived from forward edges.
    pub fn dependents(&self, key: &AssetKey) -> Result<Vec<AssetKey>, AssetError> {
        if !self.loaded.contains_key(key) {
            return Err(self.missing_loaded(key));
        }
        Ok(self
            .loaded
            .iter()
            .filter(|(_, asset)| asset.dependencies.contains(key))
            .map(|(key, _)| key.clone())
            .collect())
    }

    fn missing_loaded(&self, key: &AssetKey) -> AssetError {
        if self.catalogue.contains_key(key) {
            AssetError::NotLoaded(key.clone())
        } else {
            AssetError::UnknownAsset(key.clone())
        }
    }

    fn key_for_handle(&self, handle: &AssetHandle) -> Result<&AssetKey, AssetError> {
        handle.check()?;
        self.keys_by_instance
            .get(handle.instance_id())
            .ok_or_else(|| AccessError::ResourceMismatch.into())
    }

    fn ensure_loaded(
        &mut self,
        key: &AssetKey,
        transaction: &mut LoadTransaction,
    ) -> Result<(), AssetError> {
        if self.loaded.contains_key(key) {
            return Ok(());
        }
        if let Some(start) = transaction
            .stack
            .iter()
            .position(|ancestor| ancestor == key)
        {
            let mut cycle = transaction.stack[start..].to_vec();
            cycle.push(key.clone());
            return Err(AssetError::DependencyCycle(cycle));
        }
        let definition = self
            .catalogue
            .get(key)
            .cloned()
            .ok_or_else(|| AssetError::UnknownAsset(key.clone()))?;
        transaction.stack.push(key.clone());
        let (result, dependencies) = {
            let mut context = AssetLoadContext {
                manager: self,
                transaction,
                key,
                dependencies: BTreeSet::new(),
                failure: None,
            };
            let result = definition.loader().load(&mut context);
            // A loader cannot turn a rejected dependency acquisition/read into
            // a successful, silently incomplete prototype by swallowing Err.
            let result = match context.failure {
                Some(error) => Err(error),
                None => result,
            };
            (result, context.dependencies)
        };
        let popped = transaction.stack.pop();
        assert_eq!(popped.as_ref(), Some(key), "balanced load stack");
        let prototype = result?;
        if prototype.kind() != key.kind() {
            return Err(AssetError::KindMismatch {
                asset: key.clone(),
                actual: *prototype.kind(),
            });
        }
        let owner = self
            .resources
            .create(prototype)
            .map_err(|error| AssetError::Access(error.error))?;
        self.keys_by_instance
            .insert(owner.id().clone(), key.clone());
        self.loaded.insert(
            key.clone(),
            LoadedAsset {
                owner,
                dependencies,
            },
        );
        transaction.created.push(key.clone());
        Ok(())
    }

    fn remove_loaded(&mut self, key: &AssetKey) {
        assert!(
            !self
                .loaded
                .values()
                .any(|asset| asset.dependencies.contains(key)),
            "remove dependents before dependencies"
        );
        let loaded = self.loaded.remove(key).expect("remove only loaded assets");
        self.keys_by_instance.remove(loaded.owner.id());
        // DSA ends the resource and destroys its payload; grants retain metadata
        // only. Dependency owners are still alive while this payload is dropped.
        drop(loaded);
    }
}

impl Drop for AssetManager {
    fn drop(&mut self) {
        while !self.loaded.is_empty() {
            let key = self
                .loaded
                .keys()
                .find(|key| {
                    !self
                        .loaded
                        .values()
                        .any(|asset| asset.dependencies.contains(*key))
                })
                .expect("acyclic loaded dependency graph")
                .clone();
            self.remove_loaded(&key);
        }
    }
}

/// Restricted interface lent to a category loader during one synchronous load.
///
/// Category implementations own dependency meaning and validation; this context
/// owns acquisition, cycle detection and graph recording. It exposes neither
/// the manager nor catalogue mutation, unload, grant revocation or nested roots.
/// Every dependency error poisons the load even if the category catches it.
pub struct AssetLoadContext<'load> {
    manager: &'load mut AssetManager,
    transaction: &'load mut LoadTransaction,
    key: &'load AssetKey,
    dependencies: BTreeSet<AssetKey>,
    failure: Option<AssetError>,
}

impl AssetLoadContext<'_> {
    /// Catalogue identity of the prototype currently being interpreted.
    pub fn key(&self) -> &AssetKey {
        self.key
    }

    /// Acquires a dependency through the same manager and records one graph edge.
    /// Repeated acquisition reuses its prototype; unknown keys, cycles and grant
    /// failures reject the entire enclosing root load, not only this method.
    pub fn require(&mut self, key: &AssetKey) -> Result<AssetHandle, AssetError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self
            .manager
            .ensure_loaded(key, self.transaction)
            .and_then(|()| self.manager.handle(key));
        match &result {
            Ok(_) => {
                self.dependencies.insert(key.clone());
            }
            Err(error) => self.failure = Some(error.clone()),
        }
        result
    }

    /// Reads only a dependency acquired through this loader context.
    /// The handle must still be live and belong to this manager. Rejection
    /// poisons the enclosing load just like a failed acquisition.
    pub fn with_dependency<R>(
        &mut self,
        handle: &AssetHandle,
        read: impl for<'prototype> FnOnce(&'prototype AssetPrototype) -> R,
    ) -> Result<R, AssetError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = (|| {
            let key = self.manager.key_for_handle(handle)?;
            if !self.dependencies.contains(key) {
                return Err(AssetError::UndeclaredDependency(key.clone()));
            }
            self.manager.read(handle, read)
        })();
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
}
