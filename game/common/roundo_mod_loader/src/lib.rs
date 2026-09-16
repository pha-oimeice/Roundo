//! Deterministic discovery and validation of Mods installed below `mods/`.
//!
//! This crate deliberately knows nothing about a particular asset type. Asset
//! importers use [`LoadedMods`] for validated ownership/dependency information
//! and [`resolve_candidates`] for the common override rule.

mod resource_types;

pub use resource_types::{
    ATOMIC_VOXEL_RESOURCE_TYPE, INPUT_RESOURCE_TYPE, LoadedResourceTypes, ResourceTypeCatalog,
    ResourceTypeId, ResourceTypeLoadError, ResourceTypeLoadErrors, WEB_UI_RESOURCE_TYPE,
};

use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ModManifest {
    pub general: ManifestGeneral,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ManifestGeneral {
    pub mod_name: String,
    pub author: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Resource Type adapter 用于候选裁决的优先级，不参与 Mod 加载顺序。
    #[serde(default, alias = "load_priority")]
    pub override_priority: i32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ModId(String);

impl ModId {
    pub fn new(author: &str, mod_name: &str) -> Result<Self, LoadError> {
        if !valid_segment(author) || !valid_segment(mod_name) {
            return Err(LoadError::InvalidModId(format!("{author}.{mod_name}")));
        }
        Ok(Self(format!("{author}.{mod_name}")))
    }
}

impl fmt::Display for ModId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for ModId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Traceable identity of one declaration within a compile-time-known Resource Type.
/// The Resource Type is carried by the registry's Rust type rather than repeated
/// in this value.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResourceName {
    owner: ModId,
    local: String,
}

impl ResourceName {
    pub fn new(owner: ModId, local: impl Into<String>) -> Result<Self, ResourceReferenceError> {
        let local = local.into();
        if !valid_resource_local_name(&local) {
            return Err(ResourceReferenceError::InvalidLocalName(local));
        }
        Ok(Self { owner, local })
    }

    pub fn owner(&self) -> &ModId {
        &self.owner
    }

    pub fn local(&self) -> &str {
        &self.local
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.owner, self.local)
    }
}

#[derive(Clone, Debug)]
pub struct LoadedMod {
    pub id: ModId,
    pub root: PathBuf,
    pub dependencies: BTreeSet<ModId>,
    pub override_priority: i32,
}

/// All installed Mods, indexed by canonical Mod ID.
#[derive(Clone, Debug, Default)]
pub struct LoadedMods {
    mods: BTreeMap<ModId, LoadedMod>,
}

impl LoadedMods {
    /// Scans direct children of `root`. A directory is a Mod only when it
    /// contains a regular `manifest.toml` file; other directories are ignored.
    /// Once a manifest is present, malformed manifests remain fatal rather than
    /// silently selecting a partial Mod set.
    pub fn discover(root: impl AsRef<Path>) -> Result<Self, LoadError> {
        let root = root.as_ref();
        let mut raw = BTreeMap::new();
        if !root.exists() {
            return Ok(Self::default());
        }
        for entry in fs::read_dir(root).map_err(|e| LoadError::Io(root.into(), e))? {
            let entry = entry.map_err(|e| LoadError::Io(root.into(), e))?;
            if !entry
                .file_type()
                .map_err(|e| LoadError::Io(entry.path(), e))?
                .is_dir()
            {
                continue;
            }
            let path = entry.path();
            let manifest_path = path.join("manifest.toml");
            match fs::metadata(&manifest_path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(LoadError::Io(manifest_path, error)),
            }
            let text = fs::read_to_string(&manifest_path)
                .map_err(|e| LoadError::Io(manifest_path.clone(), e))?;
            let manifest: ModManifest = toml::from_str(&text)
                .map_err(|e| LoadError::Manifest(manifest_path, e.to_string()))?;
            let id = ModId::new(&manifest.general.author, &manifest.general.mod_name)?;
            if raw.insert(id.clone(), (path, manifest)).is_some() {
                return Err(LoadError::DuplicateModId(id));
            }
        }

        let known: BTreeSet<_> = raw.keys().cloned().collect();
        let mut mods = BTreeMap::new();
        for (id, (root, manifest)) in &raw {
            let mut dependencies = BTreeSet::new();
            for dependency in &manifest.general.dependencies {
                let dependency =
                    parse_mod_id(dependency).map_err(|_| LoadError::InvalidDependencyId {
                        mod_id: id.clone(),
                        dependency: dependency.clone(),
                    })?;
                if !known.contains(&dependency) {
                    return Err(LoadError::MissingDependency {
                        mod_id: id.clone(),
                        dependency,
                    });
                }
                dependencies.insert(dependency);
            }
            mods.insert(
                id.clone(),
                LoadedMod {
                    id: id.clone(),
                    root: root.clone(),
                    dependencies,
                    override_priority: manifest.general.override_priority,
                },
            );
        }
        let loaded = Self { mods };
        for id in loaded.mods.keys() {
            loaded.dependency_closure(id)?;
        }
        Ok(loaded)
    }

    pub fn mod_by_id(&self, id: &ModId) -> Option<&LoadedMod> {
        return self.mods.get(id);
    }
    pub fn iter(&self) -> impl Iterator<Item = &LoadedMod> {
        self.mods.values()
    }

    /// Resolves one same-Resource-Type reference without searching dependency
    /// order. Local references address `owner`; complete references must name a
    /// Mod in the owner's declared transitive dependency closure.
    pub fn resolve_resource_reference(
        &self,
        owner: &ModId,
        local_names: &BTreeSet<String>,
        reference: &str,
    ) -> Result<ResourceName, ResourceReferenceError> {
        if valid_resource_local_name(reference) {
            if !local_names.contains(reference) {
                return Err(ResourceReferenceError::UnknownLocal {
                    owner: owner.clone(),
                    local: reference.into(),
                });
            }
            return ResourceName::new(owner.clone(), reference);
        }

        let mut parts = reference.split('.');
        let author = parts.next().unwrap_or_default();
        let mod_name = parts.next().unwrap_or_default();
        let local = parts.next().unwrap_or_default();
        if parts.next().is_some() || !valid_resource_local_name(local) {
            return Err(ResourceReferenceError::InvalidReference(reference.into()));
        }
        let reference_owner = parse_mod_id(&format!("{author}.{mod_name}"))
            .map_err(|_| ResourceReferenceError::InvalidReference(reference.into()))?;
        let closure = self
            .dependency_closure(owner)
            .map_err(ResourceReferenceError::ModLoader)?;
        if !closure.contains(&reference_owner) {
            return Err(ResourceReferenceError::UndeclaredDependency {
                owner: owner.clone(),
                reference: reference.into(),
            });
        }
        ResourceName::new(reference_owner, local)
    }

    /// Returns the transitive dependency closure, excluding `id` itself.
    pub fn dependency_closure(&self, id: &ModId) -> Result<BTreeSet<ModId>, LoadError> {
        let mut closure = BTreeSet::new();
        let mut visiting = Vec::new();
        self.visit(id, &mut visiting, &mut closure)?;
        let removed_root = closure.remove(id);
        debug_assert!(removed_root, "dependency traversal must visit its root Mod");
        Ok(closure)
    }

    fn visit(
        &self,
        id: &ModId,
        visiting: &mut Vec<ModId>,
        closure: &mut BTreeSet<ModId>,
    ) -> Result<(), LoadError> {
        if let Some(start) = visiting.iter().position(|item| item == id) {
            let mut cycle = visiting[start..].to_vec();
            cycle.push(id.clone());
            return Err(LoadError::DependencyCycle(cycle));
        }
        if !closure.insert(id.clone()) {
            return Ok(());
        }
        visiting.push(id.clone());
        let item = self.mods.get(id);
        let item = item.expect("dependencies were checked during discovery");
        for dependency in &item.dependencies {
            self.visit(dependency, visiting, closure)?;
        }
        visiting.pop();
        Ok(())
    }
}

/// A proposed value for a globally exclusive resource/slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceCandidate<T> {
    pub owner: ModId,
    pub priority: i32,
    pub value: T,
}

/// Resolves candidates using the documented priority then canonical Mod ID
/// ordering. The last Mod in that ordering wins.
pub fn resolve_candidates<T>(
    candidates: impl IntoIterator<Item = ResourceCandidate<T>>,
) -> Option<ResourceCandidate<T>> {
    candidates.into_iter().max_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.owner.cmp(&b.owner))
    })
}

pub fn parse_mod_id(value: &str) -> Result<ModId, LoadError> {
    let Some((author, mod_name)) = value.split_once('.') else {
        return Err(LoadError::InvalidModId(value.into()));
    };
    if mod_name.contains('.') {
        return Err(LoadError::InvalidModId(value.into()));
    }
    ModId::new(author, mod_name)
}

fn valid_segment(value: &str) -> bool {
    let mut chars = value.bytes();
    matches!(chars.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && chars.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

pub fn valid_resource_local_name(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('.')
        && value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

#[derive(Debug)]
pub enum ResourceReferenceError {
    InvalidLocalName(String),
    UnknownLocal { owner: ModId, local: String },
    InvalidReference(String),
    UndeclaredDependency { owner: ModId, reference: String },
    ModLoader(LoadError),
}

impl fmt::Display for ResourceReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLocalName(local) => write!(f, "invalid resource local name `{local}`"),
            Self::UnknownLocal { owner, local } => {
                write!(f, "unknown local resource `{local}` in `{owner}`")
            }
            Self::InvalidReference(reference) => {
                write!(f, "invalid resource reference `{reference}`")
            }
            Self::UndeclaredDependency { owner, reference } => write!(
                f,
                "`{owner}` references resource `{reference}` without declaring its dependency"
            ),
            Self::ModLoader(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ResourceReferenceError {}

#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, std::io::Error),
    Manifest(PathBuf, String),
    InvalidModId(String),
    InvalidDependencyId { mod_id: ModId, dependency: String },
    DuplicateModId(ModId),
    MissingDependency { mod_id: ModId, dependency: ModId },
    DependencyCycle(Vec<ModId>),
}
impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "cannot read {}: {error}", path.display()),
            Self::Manifest(path, error) => {
                write!(f, "invalid manifest {}: {error}", path.display())
            }
            Self::InvalidModId(id) => write!(f, "invalid Mod ID `{id}`"),
            Self::InvalidDependencyId { mod_id, dependency } => write!(
                f,
                "dependency validation: Mod `{mod_id}` declares invalid dependency ID `{dependency}`"
            ),
            Self::DuplicateModId(id) => write!(f, "duplicate Mod ID `{id}`"),
            Self::MissingDependency { mod_id, dependency } => {
                write!(f, "Mod `{mod_id}` depends on missing Mod `{dependency}`")
            }
            Self::DependencyCycle(cycle) => write!(
                f,
                "cyclic Mod dependency: {}",
                cycle
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
        }
    }
}
impl std::error::Error for LoadError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(value: &str) -> ModId {
        parse_mod_id(value).unwrap()
    }

    fn write_fixture(path: impl AsRef<Path>, contents: &str) {
        let result = fs::write(path.as_ref(), contents);
        result.unwrap_or_else(|error| {
            panic!(
                "cannot write test fixture {}: {error}",
                path.as_ref().display()
            )
        });
    }
    #[test]
    fn priority_then_id_selects_the_winner() {
        let selected = resolve_candidates([
            ResourceCandidate {
                owner: id("z.a"),
                priority: 0,
                value: 1,
            },
            ResourceCandidate {
                owner: id("a.a"),
                priority: 1,
                value: 2,
            },
        ])
        .unwrap();
        assert_eq!(selected.value, 2);
        let selected = resolve_candidates([
            ResourceCandidate {
                owner: id("a.a"),
                priority: 0,
                value: 1,
            },
            ResourceCandidate {
                owner: id("z.a"),
                priority: 0,
                value: 2,
            },
        ])
        .unwrap();
        assert_eq!(selected.value, 2);
    }
    #[test]
    fn rejects_invalid_segments() {
        assert!(ModId::new("Author", "ui").is_err());
    }

    #[test]
    fn resolves_local_and_declared_dependency_resource_names() {
        let root = std::env::temp_dir().join(format!(
            "roundo-mod-loader-reference-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("base")).unwrap();
        fs::create_dir_all(root.join("consumer")).unwrap();
        write_fixture(
            root.join("base/manifest.toml"),
            "[general]\nmod_name='base'\nauthor='test'\n",
        );
        write_fixture(
            root.join("consumer/manifest.toml"),
            "[general]\nmod_name='consumer'\nauthor='test'\ndependencies=['test.base']\n",
        );
        let mods = LoadedMods::discover(&root).unwrap();
        let owner = id("test.consumer");
        let local = BTreeSet::from(["local".to_string()]);

        assert_eq!(
            mods.resolve_resource_reference(&owner, &local, "local")
                .unwrap()
                .to_string(),
            "test.consumer.local"
        );
        assert_eq!(
            mods.resolve_resource_reference(&owner, &local, "test.base.shared")
                .unwrap()
                .to_string(),
            "test.base.shared"
        );
        assert!(matches!(
            mods.resolve_resource_reference(&owner, &local, "other.mod.hidden"),
            Err(ResourceReferenceError::UndeclaredDependency { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn override_priority_has_a_legacy_deserialization_alias() {
        let current: ModManifest =
            toml::from_str("[general]\nmod_name='ui'\nauthor='test'\noverride_priority=7\n")
                .unwrap();
        let legacy: ModManifest =
            toml::from_str("[general]\nmod_name='ui'\nauthor='test'\nload_priority=6\n").unwrap();
        assert_eq!(current.general.override_priority, 7);
        assert_eq!(legacy.general.override_priority, 6);
    }

    #[test]
    fn reports_the_mod_and_validation_stage_for_an_invalid_dependency() {
        let root = std::env::temp_dir().join(format!(
            "roundo-mod-loader-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("broken-mod")).unwrap();
        write_fixture(
            root.join("broken-mod/manifest.toml"),
            "[general]\nmod_name = 'broken_mod'\nauthor = 'test'\ndependencies = ['base']\n",
        );

        let error = LoadedMods::discover(&root).unwrap_err();

        assert_eq!(
            error.to_string(),
            "dependency validation: Mod `test.broken_mod` declares invalid dependency ID `base`"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_directories_without_a_mod_manifest() {
        let root = std::env::temp_dir().join(format!(
            "roundo-mod-loader-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("not-a-mod/assets")).unwrap();
        fs::create_dir_all(root.join("valid-mod")).unwrap();
        write_fixture(
            root.join("valid-mod/manifest.toml"),
            "[general]\nmod_name = 'valid_mod'\nauthor = 'test'\n",
        );

        let mods = LoadedMods::discover(&root).unwrap();

        assert_eq!(mods.iter().count(), 1);
        assert!(mods.mod_by_id(&id("test.valid_mod")).is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
