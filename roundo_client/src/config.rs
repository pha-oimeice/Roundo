//! Client process configuration plus the installation-wide configuration shared with the server.

use bevy::prelude::KeyCode;
use roundo_marionette::{ClientKeyBindings, ClientMarionetteInputSettings, MovementAction};
use roundo_presence::ClientPresenceSettings;
use roundo_user_config::{
    ClientConfig, ClientKeyCode, ClientMovementAction, ClientNetworkConfig, ClientSettingsConfig,
    CommonConfig,
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

/// Resolves relative Mod paths against the directory containing the executable
/// and its configuration files, rather than the caller's working directory.
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
    }
}

pub fn runtime_key_bindings(settings: &ClientSettingsConfig) -> ClientKeyBindings {
    let mut bindings = ClientKeyBindings::empty();
    for configured in &settings.key_bindings {
        let key = runtime_key_code(configured.key);
        for action in configured.actions.iter().copied().map(runtime_action) {
            bindings.bind(key, action);
        }
    }
    bindings
}

fn read_config() -> std::sync::RwLockReadGuard<'static, ClientConfig> {
    CLIENT_CONFIG
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const KEY_CODE_PAIRS: [(ClientKeyCode, KeyCode); 52] = [
    (ClientKeyCode::Escape, KeyCode::Escape),
    (ClientKeyCode::Digit0, KeyCode::Digit0),
    (ClientKeyCode::Digit1, KeyCode::Digit1),
    (ClientKeyCode::Digit2, KeyCode::Digit2),
    (ClientKeyCode::Digit3, KeyCode::Digit3),
    (ClientKeyCode::Digit4, KeyCode::Digit4),
    (ClientKeyCode::Digit5, KeyCode::Digit5),
    (ClientKeyCode::Digit6, KeyCode::Digit6),
    (ClientKeyCode::Digit7, KeyCode::Digit7),
    (ClientKeyCode::Digit8, KeyCode::Digit8),
    (ClientKeyCode::Digit9, KeyCode::Digit9),
    (ClientKeyCode::Backspace, KeyCode::Backspace),
    (ClientKeyCode::Tab, KeyCode::Tab),
    (ClientKeyCode::KeyQ, KeyCode::KeyQ),
    (ClientKeyCode::KeyW, KeyCode::KeyW),
    (ClientKeyCode::KeyE, KeyCode::KeyE),
    (ClientKeyCode::KeyR, KeyCode::KeyR),
    (ClientKeyCode::KeyT, KeyCode::KeyT),
    (ClientKeyCode::KeyY, KeyCode::KeyY),
    (ClientKeyCode::KeyU, KeyCode::KeyU),
    (ClientKeyCode::KeyI, KeyCode::KeyI),
    (ClientKeyCode::KeyO, KeyCode::KeyO),
    (ClientKeyCode::KeyP, KeyCode::KeyP),
    (ClientKeyCode::CapsLock, KeyCode::CapsLock),
    (ClientKeyCode::KeyA, KeyCode::KeyA),
    (ClientKeyCode::KeyS, KeyCode::KeyS),
    (ClientKeyCode::KeyD, KeyCode::KeyD),
    (ClientKeyCode::KeyF, KeyCode::KeyF),
    (ClientKeyCode::KeyG, KeyCode::KeyG),
    (ClientKeyCode::KeyH, KeyCode::KeyH),
    (ClientKeyCode::KeyJ, KeyCode::KeyJ),
    (ClientKeyCode::KeyK, KeyCode::KeyK),
    (ClientKeyCode::KeyL, KeyCode::KeyL),
    (ClientKeyCode::Enter, KeyCode::Enter),
    (ClientKeyCode::ShiftLeft, KeyCode::ShiftLeft),
    (ClientKeyCode::KeyZ, KeyCode::KeyZ),
    (ClientKeyCode::KeyX, KeyCode::KeyX),
    (ClientKeyCode::KeyC, KeyCode::KeyC),
    (ClientKeyCode::KeyV, KeyCode::KeyV),
    (ClientKeyCode::KeyB, KeyCode::KeyB),
    (ClientKeyCode::KeyN, KeyCode::KeyN),
    (ClientKeyCode::KeyM, KeyCode::KeyM),
    (ClientKeyCode::ShiftRight, KeyCode::ShiftRight),
    (ClientKeyCode::ControlLeft, KeyCode::ControlLeft),
    (ClientKeyCode::AltLeft, KeyCode::AltLeft),
    (ClientKeyCode::Space, KeyCode::Space),
    (ClientKeyCode::AltRight, KeyCode::AltRight),
    (ClientKeyCode::ControlRight, KeyCode::ControlRight),
    (ClientKeyCode::ArrowLeft, KeyCode::ArrowLeft),
    (ClientKeyCode::ArrowUp, KeyCode::ArrowUp),
    (ClientKeyCode::ArrowDown, KeyCode::ArrowDown),
    (ClientKeyCode::ArrowRight, KeyCode::ArrowRight),
];

fn runtime_key_code(configured: ClientKeyCode) -> KeyCode {
    KEY_CODE_PAIRS
        .iter()
        .find_map(|(config, runtime)| (*config == configured).then_some(*runtime))
        .expect("every configured key code must have a runtime mapping")
}

fn runtime_action(configured: ClientMovementAction) -> MovementAction {
    match configured {
        ClientMovementAction::MoveUp => MovementAction::MoveUp,
        ClientMovementAction::MoveDown => MovementAction::MoveDown,
        ClientMovementAction::MoveLeft => MovementAction::MoveLeft,
        ClientMovementAction::MoveRight => MovementAction::MoveRight,
        ClientMovementAction::MoveForward => MovementAction::MoveForward,
        ClientMovementAction::MoveBackward => MovementAction::MoveBackward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dev_mode_defaults_to_false() {
        let config: ClientConfig = toml::from_str("[network]").unwrap();
        assert!(!config.dev_mode);
    }

    #[test]
    fn missing_settings_deserialize_to_the_shared_defaults() {
        let config: ClientConfig = toml::from_str(
            r#"
            [network]
            ca_verification = false
            "#,
        )
        .expect("legacy client config should deserialize");

        assert_eq!(config.settings, ClientSettingsConfig::default());
        assert_eq!(config.network.endpoint.quic_port, 12358);
    }
}
