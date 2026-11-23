//! Sole Mod initialization authority: discover, parse, validate, bind and publish.

use crate::{
    AssetAccess, AssetDefinition, AssetKey, AssetLimits, AssetManager, AssetName, LoadedMods,
    ModId,
    source::{SourceDirectory, usd_imports::ImportDirectory},
    usd::UsdSource,
};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

type ErasedPrototype = Box<dyn Any + Send + Sync>;
type NativeReceiver = Box<dyn Fn(&ModTypeInput) -> Result<ErasedPrototype, String> + Send + Sync>;

/// A class of game configuration delivered as a prototype registry, not an AssetKind.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PrototypeTypeId(pub &'static str);
pub const ATOMIC_VOXEL_PROTOTYPE_TYPE: PrototypeTypeId = PrototypeTypeId("atomic-voxel");
pub const INPUT_PROTOTYPE_TYPE: PrototypeTypeId = PrototypeTypeId("input");
pub const ITEM_PROTOTYPE_TYPE: PrototypeTypeId = PrototypeTypeId("item");

/// Loader-parsed configuration with diagnostic identity kept outside its data.
pub struct ModConfig {
    pub owner: ModId,
    pub path: PathBuf,
    pub value: toml::Table,
}
impl ModConfig {
    /// Decodes an existing game schema without filesystem access.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        self.value
            .clone()
            .try_into()
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }
}

/// Immutable data delivered to a domain's prototype receiver during initialization.
/// Receivers validate their domain, but do not scan directories or create instances.
pub struct ModTypeInput {
    mods: Arc<LoadedMods>,
    configs: Vec<ModConfig>,
}
impl ModTypeInput {
    pub fn mods(&self) -> &LoadedMods {
        &self.mods
    }
    pub fn configs(&self) -> &[ModConfig] {
        &self.configs
    }
}

struct Registration {
    dependencies: &'static [PrototypeTypeId],
    receiver: NativeReceiver,
}

/// Consumed initialization builder. There is no post-initialization add-Mod,
/// source registration or AssetManager constructor available to consumers.
pub struct ModLoader {
    registrations: BTreeMap<PrototypeTypeId, Registration>,
    duplicate: Option<PrototypeTypeId>,
    limits: AssetLimits,
    usd_sources: Vec<(AssetName, UsdSource)>,
}
impl ModLoader {
    pub fn new(limits: AssetLimits) -> Self {
        Self {
            registrations: BTreeMap::new(),
            duplicate: None,
            limits,
            usd_sources: Vec::new(),
        }
    }

    /// Registers a data receiver, not a gameplay factory.
    pub fn register_prototype<T, E>(
        mut self,
        id: PrototypeTypeId,
        dependencies: &'static [PrototypeTypeId],
        receive: fn(&ModTypeInput) -> Result<T, E>,
    ) -> Self
    where
        T: crate::ModPrototype,
        E: fmt::Display + 'static,
    {
        let receiver = Box::new(move |input: &ModTypeInput| {
            receive(input)
                .map(|value| Box::new(value) as ErasedPrototype)
                .map_err(|error| error.to_string())
        });
        self.insert(
            id,
            Registration {
                dependencies,
                receiver,
            },
        );
        self
    }

    /// Explicit initialization input until a USD configuration schema is defined.
    /// Every owner must be a discovered valid Mod. No filename-derived names exist.
    pub fn with_usd_sources(
        mut self,
        sources: impl IntoIterator<Item = (AssetName, UsdSource)>,
    ) -> Self {
        self.usd_sources.extend(sources);
        self
    }

    fn insert(&mut self, id: PrototypeTypeId, registration: Registration) {
        if self.registrations.insert(id, registration).is_some() {
            self.duplicate = Some(id);
        }
    }

    /// Initializes one closed Mod namespace and returns only prototype data and
    /// explicit asset operations. Returned errors publish no manager. Receiver
    /// side effects/panics are not transactions and are not claimed to roll back.
    pub fn initialize(
        self,
        mods_root: impl AsRef<Path>,
        roots: &[PrototypeTypeId],
    ) -> Result<ModManager, PrototypeLoadErrors> {
        if let Some(id) = self.duplicate {
            return Err(PrototypeLoadErrors::one(id, "duplicate prototype receiver"));
        }
        let selected = self.selected_types(roots)?;
        let phases = self.phases(&selected)?;
        let mods =
            Arc::new(LoadedMods::discover(mods_root).map_err(|error| {
                PrototypeLoadErrors::one(PrototypeTypeId("mod-discovery"), error)
            })?);
        let directory = SourceDirectory::new(Arc::clone(&mods))
            .map_err(|error| PrototypeLoadErrors::one(PrototypeTypeId("mod-sources"), error))?;
        let bindings: Vec<(AssetKey, Arc<AssetDefinition>)> =
            ImportDirectory::prepare(Arc::clone(&directory), self.usd_sources)
                .map_err(|error| PrototypeLoadErrors::one(PrototypeTypeId("usd"), error))?;
        let mut published = BTreeMap::new();
        for phase in phases {
            let mut staged = Vec::new();
            let mut errors = Vec::new();
            for id in phase {
                let receive = &self.registrations[&id].receiver;
                let result = read_configs(&directory, id).and_then(|input| receive(&input));
                match result {
                    Ok(prototype) => staged.push((id, prototype)),
                    Err(message) => errors.push(PrototypeLoadError { id, message }),
                }
            }
            if !errors.is_empty() {
                return Err(PrototypeLoadErrors(errors));
            }
            published.extend(staged);
        }
        let mut unique = BTreeSet::new();
        for (key, _) in &bindings {
            if !unique.insert(key.clone()) {
                return Err(PrototypeLoadErrors::one(
                    PrototypeTypeId("asset-bindings"),
                    format!("duplicate asset {key:?}"),
                ));
            }
        }
        let assets = AssetAccess::new(AssetManager::new(self.limits, bindings));
        Ok(ModManager {
            mods,
            published,
            assets,
        })
    }

    fn selected_types(
        &self,
        roots: &[PrototypeTypeId],
    ) -> Result<BTreeSet<PrototypeTypeId>, PrototypeLoadErrors> {
        let mut selected = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(id) = pending.pop() {
            if !selected.insert(id) {
                continue;
            }
            let registration = self.registrations.get(&id).ok_or_else(|| {
                PrototypeLoadErrors::one(id, "prototype receiver is not registered")
            })?;
            pending.extend(registration.dependencies.iter().copied());
        }
        Ok(selected)
    }
    fn phases(
        &self,
        selected: &BTreeSet<PrototypeTypeId>,
    ) -> Result<Vec<Vec<PrototypeTypeId>>, PrototypeLoadErrors> {
        let mut remaining = selected.clone();
        let mut loaded = BTreeSet::new();
        let mut phases = Vec::new();
        while !remaining.is_empty() {
            let ready: Vec<_> = remaining
                .iter()
                .copied()
                .filter(|id| {
                    self.registrations[id]
                        .dependencies
                        .iter()
                        .all(|dep| loaded.contains(dep))
                })
                .collect();
            if ready.is_empty() {
                return Err(PrototypeLoadErrors::one(
                    *remaining.iter().next().expect("nonempty"),
                    "cyclic prototype initialization dependencies",
                ));
            }
            for id in &ready {
                remaining.remove(id);
                loaded.insert(*id);
            }
            phases.push(ready);
        }
        Ok(phases)
    }
}

fn read_configs(
    directory: &Arc<SourceDirectory>,
    id: PrototypeTypeId,
) -> Result<ModTypeInput, String> {
    // Existing game schemas and their directories have one definition here.
    let relative = match id {
        ATOMIC_VOXEL_PROTOTYPE_TYPE => "assets/voxel/registry.toml",
        INPUT_PROTOTYPE_TYPE => "assets/input/registry.toml",
        ITEM_PROTOTYPE_TYPE => "game_data/item/registry.toml",
        _ => {
            return Err(format!(
                "no game configuration directory/schema registered for {}",
                id.0
            ));
        }
    };
    let mut configs = Vec::new();
    for installed in directory.mods.iter() {
        let path = installed.root.join(relative);
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("configuration is not a file: {}", path.display()));
            }
            Ok(_) => {}
        }
        let bound = directory.bind(&installed.id, Path::new(relative))?;
        let bytes = bound.read()?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| format!("{}: {error}", bound.path().display()))?;
        let value =
            toml::from_str(text).map_err(|error| format!("{}: {error}", bound.path().display()))?;
        configs.push(ModConfig {
            owner: installed.id.clone(),
            path: bound.path().to_path_buf(),
            value,
        });
    }
    Ok(ModTypeInput {
        mods: Arc::clone(&directory.mods),
        configs,
    })
}

/// Initialized Mod namespace. All registered values are prototypes, not instances.
/// Domain modules receive prototype registries; runtime object ownership stays there.
pub struct ModManager {
    mods: Arc<LoadedMods>,
    published: BTreeMap<PrototypeTypeId, ErasedPrototype>,
    assets: AssetAccess,
}
impl ModManager {
    pub fn mods(&self) -> &LoadedMods {
        &self.mods
    }
    pub fn assets(&self) -> AssetAccess {
        self.assets.clone()
    }
    pub fn take_prototype<T: crate::ModPrototype>(
        &mut self,
        id: PrototypeTypeId,
    ) -> Result<T, PrototypeLoadErrors> {
        // Check the type before removal; a rejected request must not lose data.
        let value = self.published.get(&id).ok_or_else(|| {
            PrototypeLoadErrors::one(
                id,
                "prototype was not published or has already been handed off",
            )
        })?;
        if !value.is::<T>() {
            return Err(PrototypeLoadErrors::one(id, "wrong prototype Rust type"));
        }
        Ok(*self
            .published
            .remove(&id)
            .expect("checked prototype")
            .downcast::<T>()
            .expect("checked prototype type"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrototypeLoadError {
    pub id: PrototypeTypeId,
    pub message: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrototypeLoadErrors(pub Vec<PrototypeLoadError>);
impl PrototypeLoadErrors {
    fn one(id: PrototypeTypeId, error: impl fmt::Display) -> Self {
        Self(vec![PrototypeLoadError {
            id,
            message: error.to_string(),
        }])
    }
}
impl fmt::Display for PrototypeLoadErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index != 0 {
                f.write_str("; ")?;
            }
            write!(f, "{}: {}", error.id.0, error.message)?;
        }
        Ok(())
    }
}
impl std::error::Error for PrototypeLoadErrors {}
