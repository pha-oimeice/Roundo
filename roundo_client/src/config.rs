//! Client configuration loading and conversion into runtime domain types.

use bevy::prelude::{KeyCode, MouseButton};
use roundo_marionette::{
    ClientInputBindings, ClientMarionetteInputSettings, InputBinding, InputRegistry, PhysicalInput,
};
use roundo_presence::ClientPresenceSettings;
use roundo_user_config::{
    ClientConfig, ClientInputKey, ClientNetworkConfig, ClientSettingsConfig, CommonConfig,
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

/// Validates configured slots against the published Mod registry and adapts
/// stable physical inputs into Bevy runtime values.
pub fn runtime_input_bindings(
    settings: &ClientSettingsConfig,
    registry: &InputRegistry,
) -> Result<ClientInputBindings, String> {
    let bindings = settings
        .input_bindings
        .iter()
        .map(|configured| {
            if !registry.contains_slot(&configured.slot) {
                return Err(format!("unknown input slot `{}`", configured.slot));
            }
            Ok(InputBinding {
                slot: configured.slot.clone(),
                input: runtime_input_key(configured.key),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ClientInputBindings::new(bindings))
}

fn read_config() -> std::sync::RwLockReadGuard<'static, ClientConfig> {
    CLIENT_CONFIG
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn runtime_input_key(configured: ClientInputKey) -> PhysicalInput {
    use ClientInputKey::*;
    let key = match configured {
        MouseLeft => return PhysicalInput::Mouse(MouseButton::Left),
        MouseRight => return PhysicalInput::Mouse(MouseButton::Right),
        MouseMiddle => return PhysicalInput::Mouse(MouseButton::Middle),
        Digit0 => KeyCode::Digit0,
        Digit1 => KeyCode::Digit1,
        Digit2 => KeyCode::Digit2,
        Digit3 => KeyCode::Digit3,
        Digit4 => KeyCode::Digit4,
        Digit5 => KeyCode::Digit5,
        Digit6 => KeyCode::Digit6,
        Digit7 => KeyCode::Digit7,
        Digit8 => KeyCode::Digit8,
        Digit9 => KeyCode::Digit9,
        Backspace => KeyCode::Backspace,
        Tab => KeyCode::Tab,
        KeyQ => KeyCode::KeyQ,
        KeyW => KeyCode::KeyW,
        KeyE => KeyCode::KeyE,
        KeyR => KeyCode::KeyR,
        KeyT => KeyCode::KeyT,
        KeyY => KeyCode::KeyY,
        KeyU => KeyCode::KeyU,
        KeyI => KeyCode::KeyI,
        KeyO => KeyCode::KeyO,
        KeyP => KeyCode::KeyP,
        CapsLock => KeyCode::CapsLock,
        KeyA => KeyCode::KeyA,
        KeyS => KeyCode::KeyS,
        KeyD => KeyCode::KeyD,
        KeyF => KeyCode::KeyF,
        KeyG => KeyCode::KeyG,
        KeyH => KeyCode::KeyH,
        KeyJ => KeyCode::KeyJ,
        KeyK => KeyCode::KeyK,
        KeyL => KeyCode::KeyL,
        Enter => KeyCode::Enter,
        ShiftLeft => KeyCode::ShiftLeft,
        KeyZ => KeyCode::KeyZ,
        KeyX => KeyCode::KeyX,
        KeyC => KeyCode::KeyC,
        KeyV => KeyCode::KeyV,
        KeyB => KeyCode::KeyB,
        KeyN => KeyCode::KeyN,
        KeyM => KeyCode::KeyM,
        ShiftRight => KeyCode::ShiftRight,
        ControlLeft => KeyCode::ControlLeft,
        AltLeft => KeyCode::AltLeft,
        Space => KeyCode::Space,
        AltRight => KeyCode::AltRight,
        ControlRight => KeyCode::ControlRight,
        ArrowLeft => KeyCode::ArrowLeft,
        ArrowUp => KeyCode::ArrowUp,
        ArrowDown => KeyCode::ArrowDown,
        ArrowRight => KeyCode::ArrowRight,
        F1 => KeyCode::F1,
    };
    PhysicalInput::Key(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve_against_builtin_input_registry() {
        let settings = ClientSettingsConfig::default();
        let bindings = runtime_input_bindings(&settings, &InputRegistry::builtin()).unwrap();
        assert_eq!(bindings.iter().count(), settings.input_bindings.len());
    }

    #[test]
    fn unknown_slots_are_rejected() {
        let mut settings = ClientSettingsConfig::default();
        settings.input_bindings[0].slot = "missing.slot".into();
        assert!(runtime_input_bindings(&settings, &InputRegistry::builtin()).is_err());
    }
}
