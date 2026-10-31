//! Client configuration loading and effective Mod/user binding resolution.

use roundo_marionette::{ClientMarionetteInputSettings, InputRegistry};
use roundo_presence::ClientPresenceSettings;
use roundo_user_config::{
    ClientConfig, ClientInputBindingConfig, ClientNetworkConfig, ClientSettingsConfig, CommonConfig,
};
use std::{
    path::PathBuf,
    sync::{LazyLock, RwLock},
};

const CONFIG_FILE_NAME: &str = "roundo-client-config.toml";
static COMMON_CONFIG: LazyLock<CommonConfig> =
    LazyLock::new(roundo_user_config::load_common_config);
static CLIENT_CONFIG: LazyLock<RwLock<ClientConfig>> = LazyLock::new(|| RwLock::new(load_config()));

pub fn load_config() -> ClientConfig {
    let mut config = roundo_user_config::load_client_config(CONFIG_FILE_NAME);
    config.settings.normalize();
    config
}
pub fn network_config() -> ClientNetworkConfig {
    read_config().network.clone()
}
pub fn mod_path() -> PathBuf {
    COMMON_CONFIG.resolved_mod_path()
}
pub fn settings() -> ClientSettingsConfig {
    read_config().settings.clone()
}
pub fn runtime_presence_settings(settings: &ClientSettingsConfig) -> ClientPresenceSettings {
    ClientPresenceSettings::new(settings.world.joinable_world_radius)
}
pub fn runtime_input_settings(settings: &ClientSettingsConfig) -> ClientMarionetteInputSettings {
    ClientMarionetteInputSettings {
        mouse_sensitivity: settings.controls.mouse_sensitivity,
        camera_move_speed: settings.camera.move_speed,
        player_movement_prediction: settings.controls.player_movement_prediction,
    }
}

/// User edges override a slot's optional Mod defaults. An explicit empty override
/// stays empty on reload; no default binding is invented by native code.
pub fn effective_input_bindings(
    settings: &ClientSettingsConfig,
    registry: Option<&InputRegistry>,
) -> Result<Vec<ClientInputBindingConfig>, String> {
    let mut bindings = settings.input_bindings.clone();
    if let Some(registry) = registry {
        for binding in &bindings {
            if !registry.contains_slot(&binding.slot) {
                return Err(format!("unknown input slot `{}`", binding.slot));
            }
        }
        for (slot, definition) in registry.slots() {
            // Validate defaults even when overridden: malformed Mod data must
            // fail startup, not remain latent until the user clears a binding.
            let defaults = definition
                .default_bindings
                .iter()
                .map(|key| {
                    serde_json::from_value(serde_json::Value::String(key.clone()))
                        .map(|key| ClientInputBindingConfig::new(slot, key))
                        .map_err(|error| format!("invalid default binding for `{slot}`: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !settings.input_binding_overrides.contains(slot)
                && !settings
                    .input_bindings
                    .iter()
                    .any(|binding| binding.slot == slot)
            {
                for binding in defaults {
                    if !bindings.contains(&binding) {
                        bindings.push(binding);
                    }
                }
            }
        }
    }
    Ok(bindings)
}

/// Materializes only the slot being edited, preserving inheritance for all others.
pub fn customize_input_slot(
    settings: &mut ClientSettingsConfig,
    registry: Option<&InputRegistry>,
    slot: &str,
) -> Result<(), String> {
    let effective = effective_input_bindings(settings, registry)?;
    settings
        .input_bindings
        .retain(|binding| binding.slot != slot);
    settings
        .input_bindings
        .extend(effective.into_iter().filter(|binding| binding.slot == slot));
    settings.input_binding_overrides.insert(slot.to_owned());
    Ok(())
}

fn read_config() -> std::sync::RwLockReadGuard<'static, ClientConfig> {
    CLIENT_CONFIG
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
