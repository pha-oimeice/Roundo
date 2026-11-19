//! Client-owned shortcut resources, references and configuration loading.

use roundo_marionette::{ClientMarionetteInputSettings, InputFunctionRegistry};
use roundo_user_config::{ClientConfig, ClientSettingsConfig, CommonConfig};
use std::{
    path::PathBuf,
    sync::{LazyLock, RwLock},
};

static COMMON_CONFIG: LazyLock<CommonConfig> =
    LazyLock::new(roundo_user_config::load_common_config);
static CLIENT_CONFIG: LazyLock<RwLock<ClientConfig>> =
    LazyLock::new(|| RwLock::new(load_config()));

pub fn load_config() -> ClientConfig {
    roundo_user_config::load_client_config()
}
pub fn mod_path() -> PathBuf {
    COMMON_CONFIG.resolved_mod_path()
}
pub fn settings() -> ClientSettingsConfig {
    read_config().settings.clone()
}
pub fn runtime_input_settings(
    settings: &ClientSettingsConfig,
) -> ClientMarionetteInputSettings {
    ClientMarionetteInputSettings {
        mouse_sensitivity: settings.controls.mouse_sensitivity,
        camera_move_speed: settings.camera.move_speed,
    }
}

/// Validates the complete reference graph before startup or publication. Nothing
/// is silently repaired, inherited or filled with native shortcut defaults.
pub fn validate_input_configuration(
    settings: &ClientSettingsConfig,
    registry: &InputFunctionRegistry,
) -> Result<(), String> {
    for (id, slot) in &settings.input_slots {
        if id.trim().is_empty() || id.trim() != id || slot.display_name.trim().is_empty() {
            return Err(format!("invalid shortcut slot `{id}` or display name"));
        }
        for function in &slot.functions {
            if registry.function(function).is_none() {
                return Err(format!("unknown function `{function}` in slot `{id}`"));
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for binding in &settings.input_bindings {
        if !settings.input_slots.contains_key(&binding.slot) {
            return Err(format!("unknown shortcut slot `{}`", binding.slot));
        }
        if !seen.insert((&binding.slot, binding.key)) {
            return Err(format!(
                "duplicate input binding for slot `{}`",
                binding.slot
            ));
        }
    }
    Ok(())
}

/// Sources resolve the same local slot resource without synthesizing a physical
/// key. The returned association set is not an execution order or permission.
pub fn resolve_slot_functions<'a>(
    settings: &'a ClientSettingsConfig,
    registry: &InputFunctionRegistry,
    slot: &str,
) -> Result<&'a std::collections::BTreeSet<String>, String> {
    let resource = settings
        .input_slots
        .get(slot)
        .ok_or_else(|| format!("unknown shortcut slot `{slot}`"))?;
    for function in &resource.functions {
        if registry.function(function).is_none() {
            return Err(format!("unknown function `{function}` in slot `{slot}`"));
        }
    }
    Ok(&resource.functions)
}

fn read_config() -> std::sync::RwLockReadGuard<'static, ClientConfig> {
    CLIENT_CONFIG
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
