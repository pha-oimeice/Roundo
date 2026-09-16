//! Mod-provided input slots and client-side physical bindings.
//!
//! The registry owns semantic input identity. Runtime bindings only connect a
//! registered slot to a physical keyboard key or mouse button; gameplay systems
//! consume slots and never inspect persisted configuration types.

use bevy::prelude::{ButtonInput, KeyCode, MouseButton, Resource};
use roundo_mod_loader::{
    LoadedMods, ModId, ResourceCandidate, ResourceName, ResourceReferenceError, resolve_candidates,
    valid_resource_local_name,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::PathBuf,
};

pub const MOVE_UP_INPUT_SLOT: &str = "roundo.move-up";
pub const MOVE_DOWN_INPUT_SLOT: &str = "roundo.move-down";
pub const MOVE_LEFT_INPUT_SLOT: &str = "roundo.move-left";
pub const MOVE_RIGHT_INPUT_SLOT: &str = "roundo.move-right";
pub const MOVE_FORWARD_INPUT_SLOT: &str = "roundo.move-forward";
pub const MOVE_BACKWARD_INPUT_SLOT: &str = "roundo.move-backward";
pub const SPIRIT_CAMERA_INPUT_SLOT: &str = "roundo.spirit-camera";
pub const DESTROY_BLOCK_INPUT_SLOT: &str = "roundo.destroy-block";
pub const PLACE_BLOCK_INPUT_SLOT: &str = "roundo.place-block";

const REQUIRED_INPUT_SLOTS: [&str; 9] = [
    MOVE_UP_INPUT_SLOT,
    MOVE_DOWN_INPUT_SLOT,
    MOVE_LEFT_INPUT_SLOT,
    MOVE_RIGHT_INPUT_SLOT,
    MOVE_FORWARD_INPUT_SLOT,
    MOVE_BACKWARD_INPUT_SLOT,
    SPIRIT_CAMERA_INPUT_SLOT,
    DESTROY_BLOCK_INPUT_SLOT,
    PLACE_BLOCK_INPUT_SLOT,
];

#[derive(Clone, Debug)]
pub struct InputDefinition {
    pub name: ResourceName,
    pub display_name: String,
}

/// Immutable Mod Resource registry for semantic input slots.
#[derive(Clone, Debug, Resource)]
pub struct InputRegistry {
    definitions: BTreeMap<ResourceName, InputDefinition>,
    slots: BTreeMap<String, ResourceName>,
}

impl InputRegistry {
    /// Loads optional `assets/input/registry.toml` declarations from every Mod.
    pub fn load(mods: &LoadedMods) -> Result<Self, InputRegistryError> {
        let mut definitions = BTreeMap::new();
        let mut candidates: BTreeMap<String, Vec<ResourceCandidate<ResourceName>>> =
            BTreeMap::new();

        for loaded in mods.iter() {
            let path = loaded.root.join("assets/input/registry.toml");
            if !path.is_file() {
                continue;
            }
            let source = fs::read_to_string(&path)
                .map_err(|error| InputRegistryError::Io(path.clone(), error))?;
            let registry: RegistryFile = toml::from_str(&source).map_err(|error| {
                InputRegistryError::InvalidRegistry(path.clone(), error.to_string())
            })?;
            let mut local_names = BTreeSet::new();
            for declaration in &registry.resources {
                if !valid_resource_local_name(&declaration.name)
                    || !local_names.insert(declaration.name.clone())
                {
                    return Err(InputRegistryError::InvalidLocalName {
                        owner: loaded.id.clone(),
                        name: declaration.name.clone(),
                    });
                }
                if declaration.display_name.trim().is_empty() {
                    return Err(InputRegistryError::EmptyDisplayName {
                        owner: loaded.id.clone(),
                        name: declaration.name.clone(),
                    });
                }
            }
            for declaration in registry.resources {
                let name = ResourceName::new(loaded.id.clone(), declaration.name)
                    .map_err(InputRegistryError::Reference)?;
                definitions.insert(
                    name.clone(),
                    InputDefinition {
                        name,
                        display_name: declaration.display_name,
                    },
                );
            }
            for (slot, reference) in registry.slots {
                if !valid_slot(&slot) {
                    return Err(InputRegistryError::InvalidSlot(slot));
                }
                let name = mods
                    .resolve_resource_reference(&loaded.id, &local_names, &reference)
                    .map_err(InputRegistryError::Reference)?;
                candidates.entry(slot).or_default().push(ResourceCandidate {
                    owner: loaded.id.clone(),
                    priority: loaded.override_priority,
                    value: name,
                });
            }
        }

        let mut slots = BTreeMap::new();
        for (slot, candidates) in candidates {
            let selected =
                resolve_candidates(candidates).expect("an input slot group is non-empty");
            if !definitions.contains_key(&selected.value) {
                return Err(InputRegistryError::UnknownResource(selected.value));
            }
            slots.insert(slot, selected.value);
        }
        for required in REQUIRED_INPUT_SLOTS {
            if !slots.contains_key(required) {
                return Err(InputRegistryError::MissingRequiredSlot(required));
            }
        }
        Ok(Self { definitions, slots })
    }

    /// Fixed registry used by focused tests and embedders.
    pub fn builtin() -> Self {
        let owner = ModId::new("vanilla", "base").expect("built-in Mod ID is valid");
        let declarations = [
            (MOVE_UP_INPUT_SLOT, "move-up", "Move Up"),
            (MOVE_DOWN_INPUT_SLOT, "move-down", "Move Down"),
            (MOVE_LEFT_INPUT_SLOT, "move-left", "Move Left"),
            (MOVE_RIGHT_INPUT_SLOT, "move-right", "Move Right"),
            (MOVE_FORWARD_INPUT_SLOT, "move-forward", "Move Forward"),
            (MOVE_BACKWARD_INPUT_SLOT, "move-backward", "Move Backward"),
            (SPIRIT_CAMERA_INPUT_SLOT, "spirit-camera", "Spirit Camera"),
            (DESTROY_BLOCK_INPUT_SLOT, "destroy-block", "Destroy Block"),
            (PLACE_BLOCK_INPUT_SLOT, "place-block", "Place Block"),
        ];
        let mut definitions = BTreeMap::new();
        let mut slots = BTreeMap::new();
        for (slot, local, display_name) in declarations {
            let name = ResourceName::new(owner.clone(), local)
                .expect("built-in input resource name is valid");
            definitions.insert(
                name.clone(),
                InputDefinition {
                    name: name.clone(),
                    display_name: display_name.into(),
                },
            );
            slots.insert(slot.into(), name);
        }
        Self { definitions, slots }
    }

    pub fn contains_slot(&self, slot: &str) -> bool {
        self.slots.contains_key(slot)
    }

    pub fn slot(&self, slot: &str) -> Option<&InputDefinition> {
        self.slots
            .get(slot)
            .and_then(|name| self.definitions.get(name))
    }

    pub fn slots(&self) -> impl Iterator<Item = (&str, &InputDefinition)> {
        self.slots.iter().filter_map(|(slot, name)| {
            self.definitions
                .get(name)
                .map(|definition| (slot.as_str(), definition))
        })
    }
}

impl Default for InputRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicalInput {
    Key(KeyCode),
    Mouse(MouseButton),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputBinding {
    pub slot: String,
    pub input: PhysicalInput,
}

/// Many-to-many slot/physical-input edges consumed by client systems.
#[derive(Clone, Debug, Default, Resource)]
pub struct ClientInputBindings {
    bindings: Vec<InputBinding>,
}

impl ClientInputBindings {
    pub fn new(bindings: impl IntoIterator<Item = InputBinding>) -> Self {
        let mut unique = Vec::new();
        for binding in bindings {
            if !unique.contains(&binding) {
                unique.push(binding);
            }
        }
        Self { bindings: unique }
    }

    pub fn iter(&self) -> impl Iterator<Item = &InputBinding> {
        self.bindings.iter()
    }

    pub fn pressed(
        &self,
        slot: &str,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        self.bindings.iter().any(|binding| {
            binding.slot == slot
                && match binding.input {
                    PhysicalInput::Key(key) => keyboard.pressed(key),
                    PhysicalInput::Mouse(button) => mouse.pressed(button),
                }
        })
    }

    pub fn just_pressed(
        &self,
        slot: &str,
        keyboard: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        self.bindings.iter().any(|binding| {
            binding.slot == slot
                && match binding.input {
                    PhysicalInput::Key(key) => keyboard.just_pressed(key),
                    PhysicalInput::Mouse(button) => mouse.just_pressed(button),
                }
        })
    }
}

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default, rename = "resource")]
    resources: Vec<RegistryInput>,
    #[serde(default)]
    slots: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RegistryInput {
    name: String,
    display_name: String,
}

fn valid_slot(slot: &str) -> bool {
    let mut parts = slot.split('.');
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None)
        if valid_resource_local_name(a) && valid_resource_local_name(b))
}

#[derive(Debug)]
pub enum InputRegistryError {
    Io(PathBuf, std::io::Error),
    InvalidRegistry(PathBuf, String),
    InvalidLocalName { owner: ModId, name: String },
    EmptyDisplayName { owner: ModId, name: String },
    InvalidSlot(String),
    MissingRequiredSlot(&'static str),
    UnknownResource(ResourceName),
    Reference(ResourceReferenceError),
}

impl fmt::Display for InputRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(formatter, "cannot read {}: {error}", path.display()),
            Self::InvalidRegistry(path, error) => {
                write!(
                    formatter,
                    "invalid input registry {}: {error}",
                    path.display()
                )
            }
            Self::InvalidLocalName { owner, name } => write!(
                formatter,
                "invalid or duplicate input resource `{name}` in `{owner}`"
            ),
            Self::EmptyDisplayName { owner, name } => write!(
                formatter,
                "input resource `{owner}.{name}` has an empty display name"
            ),
            Self::InvalidSlot(slot) => write!(formatter, "invalid input slot `{slot}`"),
            Self::MissingRequiredSlot(slot) => {
                write!(formatter, "missing required input slot `{slot}`")
            }
            Self::UnknownResource(name) => write!(formatter, "unknown input resource `{name}`"),
            Self::Reference(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InputRegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roundo-input-registry-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mod_root = root.join("base");
        std::fs::create_dir_all(mod_root.join("assets/input")).unwrap();
        std::fs::write(
            mod_root.join("manifest.toml"),
            "[general]\nmod_name = \"base\"\nauthor = \"vanilla\"\n",
        )
        .unwrap();
        let builtin = InputRegistry::builtin();
        // TOML does not permit reopening the same table, so collect slots separately.
        let mut resources = String::new();
        let mut slots = String::from("[slots]\n");
        for (slot, definition) in builtin.slots() {
            resources.push_str(&format!(
                "[[resource]]\nname = \"{}\"\ndisplay_name = \"{}\"\n",
                definition.name.local(),
                definition.display_name
            ));
            slots.push_str(&format!("\"{slot}\" = \"{}\"\n", definition.name.local()));
        }
        std::fs::write(
            mod_root.join("assets/input/registry.toml"),
            format!("{resources}{slots}"),
        )
        .unwrap();
        root
    }

    #[test]
    fn loads_registered_slots_from_mod_resources() {
        let root = fixture();
        let mods = LoadedMods::discover(&root).unwrap();
        let registry = InputRegistry::load(&mods).unwrap();
        assert_eq!(registry.slots().count(), REQUIRED_INPUT_SLOTS.len());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builtin_registry_publishes_every_controller_slot() {
        let registry = InputRegistry::builtin();
        for slot in REQUIRED_INPUT_SLOTS {
            assert!(registry.contains_slot(slot));
        }
    }

    #[test]
    fn bindings_are_many_to_many_and_deduplicated() {
        let binding = InputBinding {
            slot: MOVE_FORWARD_INPUT_SLOT.into(),
            input: PhysicalInput::Key(KeyCode::KeyW),
        };
        let bindings = ClientInputBindings::new([
            binding.clone(),
            binding,
            InputBinding {
                slot: MOVE_UP_INPUT_SLOT.into(),
                input: PhysicalInput::Key(KeyCode::KeyW),
            },
        ]);
        assert_eq!(bindings.iter().count(), 2);
    }
}
