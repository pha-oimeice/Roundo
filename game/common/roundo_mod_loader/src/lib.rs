//! Deterministic discovery and validation of Mods installed below `mods/`.
//!
//! This crate deliberately knows nothing about a particular asset type. Asset
//! importers use [`LoadedMods`] for validated ownership/dependency information
//! and [`resolve_candidates`] for the common override rule.

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
    #[serde(default)]
    pub load_priority: i32,
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

#[derive(Clone, Debug)]
pub struct LoadedMod {
    pub id: ModId,
    pub root: PathBuf,
    pub dependencies: BTreeSet<ModId>,
    pub load_priority: i32,
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
                    load_priority: manifest.general.load_priority,
                },
            );
        }
        let loaded = Self { mods };
        for id in loaded.mods.keys() {
            loaded.dependency_closure(id)?;
        }
        Ok(loaded)
    }

    pub fn get(&self, id: &ModId) -> Option<&LoadedMod> {
        self.mods.get(id)
    }
    pub fn iter(&self) -> impl Iterator<Item = &LoadedMod> {
        self.mods.values()
    }

    /// Returns the transitive dependency closure, excluding `id` itself.
    pub fn dependency_closure(&self, id: &ModId) -> Result<BTreeSet<ModId>, LoadError> {
        let mut closure = BTreeSet::new();
        let mut visiting = Vec::new();
        self.visit(id, &mut visiting, &mut closure)?;
        closure.remove(id);
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
        let item = self
            .mods
            .get(id)
            .expect("dependencies were checked during discovery");
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
    fn reports_the_mod_and_validation_stage_for_an_invalid_dependency() {
        let root = std::env::temp_dir().join(format!(
            "roundo-mod-loader-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("broken-mod")).unwrap();
        fs::write(
            root.join("broken-mod/manifest.toml"),
            "[general]\nmod_name = 'broken_mod'\nauthor = 'test'\ndependencies = ['base']\n",
        )
        .unwrap();

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
        fs::write(
            root.join("valid-mod/manifest.toml"),
            "[general]\nmod_name = 'valid_mod'\nauthor = 'test'\n",
        )
        .unwrap();

        let mods = LoadedMods::discover(&root).unwrap();

        assert_eq!(mods.iter().count(), 1);
        assert!(mods.get(&id("test.valid_mod")).is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
