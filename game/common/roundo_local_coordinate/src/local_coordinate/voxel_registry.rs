//! Immutable Mod Resource registry for atomic voxel identities.
//!
//! The registry is the seam between traceable Mod Resource Names and compact
//! wire/storage IDs. Chunk storage, PCG algorithms, physics, and rendering only
//! consume validated IDs and do not parse Mod files.

use crate::local_coordinate::data::{
    AtomicVoxelId, EMPTY_VOXEL_ID, GlobalAtomicVoxelData, SOLID_VOXEL_ID,
};
use bevy::prelude::Resource;
use roundo_mod_loader::{
    LoadedMods, ModId, ResourceCandidate, ResourceName, ResourceReferenceError, resolve_candidates,
    valid_resource_local_name,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::PathBuf,
};

/// Slot selecting the voxel emitted by the default placement action.
pub const DEFAULT_PLACED_VOXEL_SLOT: &str = "roundo.default-placed-voxel";
/// Slot mapping nonzero procedural material values to a voxel definition.
pub const GENERATED_SOLID_VOXEL_SLOT: &str = "roundo.generated-solid-voxel";

/// Immutable mapping from traceable voxel resources to compact runtime IDs.
///
/// ID zero is reserved for empty space and never has a definition. Cloning
/// copies the published maps; later lookups on either value remain independent.
#[derive(Clone, Debug, Resource)]
pub struct AtomicVoxelRegistry {
    definitions: BTreeMap<AtomicVoxelId, GlobalAtomicVoxelData>,
    ids_by_name: BTreeMap<ResourceName, AtomicVoxelId>,
    slots: BTreeMap<String, AtomicVoxelId>,
    fingerprint: [u8; 32],
}

impl AtomicVoxelRegistry {
    /// Loads `assets/voxel/registry.toml` from every discovered Mod.
    ///
    /// Candidate slots are resolved by override priority and then canonical Mod
    /// ID. The returned registry is published only after every declaration,
    /// reference, compact ID, required slot, and placement constraint validates.
    /// This performs synchronous filesystem I/O.
    ///
    /// # Errors
    ///
    /// Returns [`AtomicVoxelRegistryError`] for an unreadable or malformed file,
    /// invalid or duplicate identity, unresolved reference, invalid slot, or a
    /// missing/invalid required slot. No partial registry is returned.
    pub fn load(mods: &LoadedMods) -> Result<Self, AtomicVoxelRegistryError> {
        let mut definitions: BTreeMap<AtomicVoxelId, GlobalAtomicVoxelData> = BTreeMap::new();
        let mut ids_by_name: BTreeMap<ResourceName, AtomicVoxelId> = BTreeMap::new();
        let mut candidates: BTreeMap<String, Vec<ResourceCandidate<ResourceName>>> =
            BTreeMap::new();

        for loaded in mods.iter() {
            let path = loaded.root.join("assets/voxel/registry.toml");
            if !path.is_file() {
                continue;
            }
            let source = fs::read_to_string(&path)
                .map_err(|error| AtomicVoxelRegistryError::Io(path.clone(), error))?;
            let registry: RegistryFile = toml::from_str(&source).map_err(|error| {
                AtomicVoxelRegistryError::InvalidRegistry(path.clone(), error.to_string())
            })?;
            let mut local_names = BTreeSet::new();
            for declaration in &registry.resources {
                if !valid_resource_local_name(&declaration.name)
                    || !local_names.insert(declaration.name.clone())
                {
                    return Err(AtomicVoxelRegistryError::InvalidLocalName {
                        owner: loaded.id.clone(),
                        name: declaration.name.clone(),
                    });
                }
            }
            for declaration in registry.resources {
                if declaration.id == EMPTY_VOXEL_ID.0 {
                    return Err(AtomicVoxelRegistryError::ReservedEmptyId {
                        owner: loaded.id.clone(),
                        name: declaration.name,
                    });
                }
                let name = ResourceName::new(loaded.id.clone(), declaration.name)
                    .map_err(AtomicVoxelRegistryError::Reference)?;
                let id = AtomicVoxelId(declaration.id);
                if let Some(existing) = definitions.get(&id) {
                    return Err(AtomicVoxelRegistryError::DuplicateId {
                        id,
                        first: existing.name.clone(),
                        second: name,
                    });
                }
                definitions.insert(
                    id,
                    GlobalAtomicVoxelData {
                        name: name.clone(),
                        placeable: declaration.placeable,
                    },
                );
                ids_by_name.insert(name, id);
            }
            for (slot, reference) in registry.slots {
                if !valid_slot(&slot) {
                    return Err(AtomicVoxelRegistryError::InvalidSlot(slot));
                }
                let name = mods
                    .resolve_resource_reference(&loaded.id, &local_names, &reference)
                    .map_err(AtomicVoxelRegistryError::Reference)?;
                candidates.entry(slot).or_default().push(ResourceCandidate {
                    owner: loaded.id.clone(),
                    priority: loaded.override_priority,
                    value: name,
                });
            }
        }

        let mut slots = BTreeMap::new();
        for (slot, candidates) in candidates {
            let selected = resolve_candidates(candidates).expect("a slot group is non-empty");
            let selected_id = ids_by_name.get(&selected.value);
            let id = selected_id
                .copied()
                .ok_or_else(|| AtomicVoxelRegistryError::UnknownResource(selected.value.clone()))?;
            slots.insert(slot, id);
        }
        for required in [DEFAULT_PLACED_VOXEL_SLOT, GENERATED_SOLID_VOXEL_SLOT] {
            if !slots.contains_key(required) {
                return Err(AtomicVoxelRegistryError::MissingRequiredSlot(required));
            }
        }
        let placed = slots[DEFAULT_PLACED_VOXEL_SLOT];
        if !definitions[&placed].placeable {
            return Err(AtomicVoxelRegistryError::DefaultVoxelNotPlaceable(
                definitions[&placed].name.clone(),
            ));
        }

        let fingerprint = fingerprint(&definitions, &slots);
        Ok(Self {
            definitions,
            ids_by_name,
            slots,
            fingerprint,
        })
    }

    /// Creates the fixed built-in registry used by focused tests and embedders.
    ///
    /// It contains one placeable `vanilla.base.stone` voxel under both required
    /// slots. Production composition roots replace this fallback with
    /// [`Self::load`].
    pub fn builtin() -> Self {
        let name = ResourceName::new(
            ModId::new("vanilla", "base").expect("built-in Mod ID is valid"),
            "stone",
        )
        .expect("built-in voxel name is valid");
        let definition = GlobalAtomicVoxelData {
            name: name.clone(),
            placeable: true,
        };
        let definitions = BTreeMap::from([(SOLID_VOXEL_ID, definition)]);
        let ids_by_name = BTreeMap::from([(name, SOLID_VOXEL_ID)]);
        let slots = BTreeMap::from([
            (DEFAULT_PLACED_VOXEL_SLOT.to_string(), SOLID_VOXEL_ID),
            (GENERATED_SOLID_VOXEL_SLOT.to_string(), SOLID_VOXEL_ID),
        ]);
        let fingerprint = fingerprint(&definitions, &slots);
        Self {
            definitions,
            ids_by_name,
            slots,
            fingerprint,
        }
    }

    /// Borrows the definition for a registered nonempty ID.
    ///
    /// Returns `None` for empty space and unknown IDs.
    pub fn definition(&self, id: AtomicVoxelId) -> Option<&GlobalAtomicVoxelData> {
        self.definitions.get(&id)
    }

    /// Resolves a traceable resource name to its compact ID.
    pub fn id(&self, name: &ResourceName) -> Option<AtomicVoxelId> {
        self.ids_by_name.get(name).copied()
    }

    /// Resolves the winning resource selected for a slot.
    pub fn slot(&self, slot: &str) -> Option<AtomicVoxelId> {
        self.slots.get(slot).copied()
    }

    /// Returns the validated nonempty voxel selected for default placement.
    pub fn default_placed_voxel(&self) -> AtomicVoxelId {
        self.slots[DEFAULT_PLACED_VOXEL_SLOT]
    }

    /// Current generators emit binary material IDs: zero is empty and every
    /// non-zero value means the selected generated-solid voxel resource.
    pub fn generated_voxel(&self, material_id: u16) -> AtomicVoxelId {
        if material_id == 0 {
            EMPTY_VOXEL_ID
        } else {
            self.slots[GENERATED_SOLID_VOXEL_SLOT]
        }
    }

    /// Returns whether `id` names a registered placeable voxel.
    ///
    /// Empty space and unknown IDs are not placeable.
    pub fn permits_placement(&self, id: AtomicVoxelId) -> bool {
        self.definition(id)
            .is_some_and(|definition| definition.placeable)
    }

    /// Returns the deterministic digest of all definitions and selected slots.
    ///
    /// Equal registries produce equal digests regardless of declaration or Mod
    /// discovery order. Session establishment uses this value for equality, not
    /// as a persistent registry identifier.
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

impl Default for AtomicVoxelRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default, rename = "resource")]
    resources: Vec<RegistryVoxel>,
    #[serde(default)]
    slots: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RegistryVoxel {
    name: String,
    id: u32,
    #[serde(default)]
    placeable: bool,
}

fn valid_slot(slot: &str) -> bool {
    let mut parts = slot.split('.');
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None)
        if valid_resource_local_name(a) && valid_resource_local_name(b))
}

fn fingerprint(
    definitions: &BTreeMap<AtomicVoxelId, GlobalAtomicVoxelData>,
    slots: &BTreeMap<String, AtomicVoxelId>,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    for (id, definition) in definitions {
        hash.update(id.0.to_le_bytes());
        hash.update(definition.name.to_string().as_bytes());
        hash.update([u8::from(definition.placeable)]);
    }
    for (slot, id) in slots {
        hash.update(slot.as_bytes());
        hash.update(id.0.to_le_bytes());
    }
    hash.finalize().into()
}

/// Failure encountered before an atomic voxel registry can be published.
#[derive(Debug)]
pub enum AtomicVoxelRegistryError {
    Io(PathBuf, std::io::Error),
    InvalidRegistry(PathBuf, String),
    InvalidLocalName {
        owner: ModId,
        name: String,
    },
    ReservedEmptyId {
        owner: ModId,
        name: String,
    },
    DuplicateId {
        id: AtomicVoxelId,
        first: ResourceName,
        second: ResourceName,
    },
    InvalidSlot(String),
    MissingRequiredSlot(&'static str),
    UnknownResource(ResourceName),
    DefaultVoxelNotPlaceable(ResourceName),
    Reference(ResourceReferenceError),
}

impl fmt::Display for AtomicVoxelRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "cannot read {}: {error}", path.display()),
            Self::InvalidRegistry(path, error) => {
                write!(f, "invalid voxel registry {}: {error}", path.display())
            }
            Self::InvalidLocalName { owner, name } => {
                write!(
                    f,
                    "invalid or duplicate voxel resource `{name}` in `{owner}`"
                )
            }
            Self::ReservedEmptyId { owner, name } => write!(
                f,
                "voxel resource `{owner}.{name}` uses reserved empty voxel ID 0"
            ),
            Self::DuplicateId { id, first, second } => write!(
                f,
                "voxel ID {} is declared by both `{first}` and `{second}`",
                id.0
            ),
            Self::InvalidSlot(slot) => write!(f, "invalid voxel slot `{slot}`"),
            Self::MissingRequiredSlot(slot) => write!(f, "missing required voxel slot `{slot}`"),
            Self::UnknownResource(name) => write!(f, "unknown voxel resource `{name}`"),
            Self::DefaultVoxelNotPlaceable(name) => {
                write!(f, "default placed voxel `{name}` is not placeable")
            }
            Self::Reference(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AtomicVoxelRegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roundo-voxel-registry-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for (relative, contents) in files {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let write_result = fs::write(path, contents);
            write_result.unwrap();
        }
        root
    }

    fn load(root: &Path) -> Result<AtomicVoxelRegistry, AtomicVoxelRegistryError> {
        let mods = LoadedMods::discover(root).unwrap();
        AtomicVoxelRegistry::load(&mods)
    }

    #[test]
    fn publishes_traceable_voxels_and_selected_slots() {
        let root = fixture(&[
            (
                "base/manifest.toml",
                "[general]\nmod_name='base'\nauthor='vanilla'\n",
            ),
            (
                "base/assets/voxel/registry.toml",
                "[[resource]]\nname='stone'\nid=7\nplaceable=true\n[slots]\n'roundo.default-placed-voxel'='stone'\n'roundo.generated-solid-voxel'='stone'\n",
            ),
        ]);
        let registry = load(&root).unwrap();
        let id = AtomicVoxelId(7);

        assert_eq!(registry.default_placed_voxel(), id);
        assert_eq!(registry.generated_voxel(1), id);
        assert_eq!(registry.generated_voxel(0), EMPTY_VOXEL_ID);
        assert_eq!(
            registry.definition(id).unwrap().name.to_string(),
            "vanilla.base.stone"
        );
        assert!(registry.permits_placement(id));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_duplicate_wire_ids_before_publication() {
        let root = fixture(&[
            (
                "a/manifest.toml",
                "[general]\nmod_name='a'\nauthor='test'\n",
            ),
            (
                "a/assets/voxel/registry.toml",
                "[[resource]]\nname='one'\nid=9\nplaceable=true\n[slots]\n'roundo.default-placed-voxel'='one'\n'roundo.generated-solid-voxel'='one'\n",
            ),
            (
                "b/manifest.toml",
                "[general]\nmod_name='b'\nauthor='test'\n",
            ),
            (
                "b/assets/voxel/registry.toml",
                "[[resource]]\nname='two'\nid=9\n",
            ),
        ]);
        let error = load(&root).unwrap_err().to_string();

        assert!(error.contains("voxel ID 9"));
        assert!(error.contains("test.a.one"));
        assert!(error.contains("test.b.two"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_references_use_the_shared_resolver() {
        let root = fixture(&[
            (
                "base/manifest.toml",
                "[general]\nmod_name='base'\nauthor='vanilla'\n",
            ),
            (
                "base/assets/voxel/registry.toml",
                "[[resource]]\nname='stone'\nid=1\nplaceable=true\n",
            ),
            (
                "choice/manifest.toml",
                "[general]\nmod_name='choice'\nauthor='test'\ndependencies=['vanilla.base']\n",
            ),
            (
                "choice/assets/voxel/registry.toml",
                "[slots]\n'roundo.default-placed-voxel'='vanilla.base.stone'\n'roundo.generated-solid-voxel'='vanilla.base.stone'\n",
            ),
        ]);
        let registry = load(&root).unwrap();

        assert_eq!(registry.default_placed_voxel(), SOLID_VOXEL_ID);
        fs::remove_dir_all(root).unwrap();
    }
}
