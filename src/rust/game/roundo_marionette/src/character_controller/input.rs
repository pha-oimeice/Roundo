//! Named source functions and semantic function input. Client shortcut slots
//! and physical bindings are separate, user-owned configuration resources.

use bevy::prelude::{Message, Resource, Vec2};
use roundo_mod_manager::{
    ModTypeInput, ResourceCandidate, ResourceName, resolve_candidates,
    valid_resource_local_name,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const MOVE_UP_FUNCTION: &str = "roundo.move-up";
pub const MOVE_DOWN_FUNCTION: &str = "roundo.move-down";
pub const MOVE_LEFT_FUNCTION: &str = "roundo.move-left";
pub const MOVE_RIGHT_FUNCTION: &str = "roundo.move-right";
pub const MOVE_FORWARD_FUNCTION: &str = "roundo.move-forward";
pub const MOVE_BACKWARD_FUNCTION: &str = "roundo.move-backward";
pub const LOOK_FUNCTION: &str = "roundo.look";

#[derive(Clone, Debug)]
pub struct InputFunctionDefinition {
    pub resource: ResourceName,
    pub display_name: String,
}

/// Mod declarations name functions implemented by their source consumers.
/// Registration describes availability, not execution or use permission; there
/// are no required shortcut slots, default bindings or invented item functions.
#[derive(Clone, Debug, Default, Resource)]
pub struct InputFunctionRegistry {
    definitions: BTreeMap<ResourceName, InputFunctionDefinition>,
    functions: BTreeMap<String, ResourceName>,
}

impl roundo_mod_manager::ModPrototype for InputFunctionRegistry {}

impl InputFunctionRegistry {
    pub fn from_mod_configs(input: &ModTypeInput) -> Result<Self, InputFunctionRegistryError> {
        let mods = input.mods();
        let mut definitions = BTreeMap::new();
        let mut candidates: BTreeMap<String, Vec<ResourceCandidate<ResourceName>>> =
            BTreeMap::new();
        for config in input.configs() {
            let loaded = mods
                .mod_by_id(&config.owner)
                .expect("loader-owned configuration");
            let registry: RegistryFile = config.decode().map_err(InputFunctionRegistryError)?;
            let mut local_names = BTreeSet::new();
            for declaration in &registry.resources {
                if !valid_resource_local_name(&declaration.name)
                    || !local_names.insert(declaration.name.clone())
                    || declaration.display_name.trim().is_empty()
                {
                    return Err(InputFunctionRegistryError(format!(
                        "invalid function resource `{}` in `{}`",
                        declaration.name, loaded.id
                    )));
                }
            }
            for declaration in registry.resources {
                let resource = ResourceName::new(loaded.id.clone(), declaration.name)
                    .map_err(|error| InputFunctionRegistryError(error.to_string()))?;
                definitions.insert(
                    resource.clone(),
                    InputFunctionDefinition {
                        resource,
                        display_name: declaration.display_name,
                    },
                );
            }
            for (function, reference) in registry.functions {
                if function.trim().is_empty() || function.trim() != function {
                    return Err(InputFunctionRegistryError(format!(
                        "invalid function name `{function}`"
                    )));
                }
                let resource = mods
                    .resolve_resource_reference(&loaded.id, &local_names, &reference)
                    .map_err(|error| InputFunctionRegistryError(error.to_string()))?;
                candidates
                    .entry(function)
                    .or_default()
                    .push(ResourceCandidate {
                        owner: loaded.id.clone(),
                        priority: loaded.override_priority,
                        value: resource,
                    });
            }
        }
        let mut functions = BTreeMap::new();
        for (function, candidates) in candidates {
            let selected =
                resolve_candidates(candidates).expect("function candidate group is nonempty");
            if !definitions.contains_key(&selected.value) {
                return Err(InputFunctionRegistryError(format!(
                    "unknown function resource `{}`",
                    selected.value
                )));
            }
            functions.insert(function, selected.value);
        }
        Ok(Self {
            definitions,
            functions,
        })
    }

    pub fn function(&self, name: &str) -> Option<&InputFunctionDefinition> {
        self.functions
            .get(name)
            .and_then(|resource| self.definitions.get(resource))
    }

    pub fn functions(&self) -> impl Iterator<Item = (&str, &InputFunctionDefinition)> {
        self.functions
            .iter()
            .map(|(name, resource)| (name.as_str(), &self.definitions[resource]))
    }
}

/// Source-function states after client slots have been resolved. Consumers do
/// not know user slot identities or physical devices. Stateful native functions
/// aggregate all active slots; future item-use execution is not defined here.
#[derive(Clone, Debug, Default, Message)]
pub struct ClientFunctionFrame {
    pub held: BTreeSet<String>,
    pub pressed: BTreeSet<String>,
    pub released: BTreeSet<String>,
    pub motion: BTreeMap<String, Vec2>,
}

impl ClientFunctionFrame {
    pub fn pressed(&self, function: &str) -> bool {
        self.held.contains(function)
    }

    pub fn just_pressed(&self, function: &str) -> bool {
        self.pressed.contains(function)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    #[serde(default, rename = "resource")]
    resources: Vec<RegistryFunction>,
    #[serde(default)]
    functions: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFunction {
    name: String,
    display_name: String,
}

#[derive(Debug)]
pub struct InputFunctionRegistryError(String);
impl fmt::Display for InputFunctionRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
impl std::error::Error for InputFunctionRegistryError {}
