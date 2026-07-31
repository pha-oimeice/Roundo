use serde::{Deserialize, Serialize};

pub const MIN_MOUSE_SENSITIVITY: f32 = 0.0005;
pub const MAX_MOUSE_SENSITIVITY: f32 = 0.01;
pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 0.002;
pub const MIN_CAMERA_MOVE_SPEED: f32 = 1.0;
pub const MAX_CAMERA_MOVE_SPEED: f32 = 50.0;
pub const DEFAULT_CAMERA_MOVE_SPEED: f32 = 5.0;
pub const MIN_VOXEL_RAYCAST_DISTANCE: f32 = 1.0;
pub const MAX_VOXEL_RAYCAST_DISTANCE: f32 = 16.0;
pub const DEFAULT_VOXEL_RAYCAST_DISTANCE: f32 = 8.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettingsConfig {
    pub controls: ClientControlSettingsConfig,
    pub camera: ClientCameraSettingsConfig,
    pub key_bindings: Vec<ClientKeyBindingConfig>,
}

impl ClientSettingsConfig {
    pub fn normalize(&mut self) {
        self.controls.mouse_sensitivity = finite_clamped(
            self.controls.mouse_sensitivity,
            DEFAULT_MOUSE_SENSITIVITY,
            MIN_MOUSE_SENSITIVITY,
            MAX_MOUSE_SENSITIVITY,
        );
        self.camera.move_speed = finite_clamped(
            self.camera.move_speed,
            DEFAULT_CAMERA_MOVE_SPEED,
            MIN_CAMERA_MOVE_SPEED,
            MAX_CAMERA_MOVE_SPEED,
        );
        self.camera.voxel_raycast_distance = finite_clamped(
            self.camera.voxel_raycast_distance,
            DEFAULT_VOXEL_RAYCAST_DISTANCE,
            MIN_VOXEL_RAYCAST_DISTANCE,
            MAX_VOXEL_RAYCAST_DISTANCE,
        );
    }
}

impl Default for ClientSettingsConfig {
    fn default() -> Self {
        Self {
            controls: ClientControlSettingsConfig::default(),
            camera: ClientCameraSettingsConfig::default(),
            key_bindings: vec![
                ClientKeyBindingConfig::new(ClientKeyCode::Space, ClientMovementAction::MoveUp),
                ClientKeyBindingConfig::new(
                    ClientKeyCode::ShiftLeft,
                    ClientMovementAction::MoveDown,
                ),
                ClientKeyBindingConfig::new(ClientKeyCode::KeyA, ClientMovementAction::MoveLeft),
                ClientKeyBindingConfig::new(ClientKeyCode::KeyD, ClientMovementAction::MoveRight),
                ClientKeyBindingConfig::new(ClientKeyCode::KeyW, ClientMovementAction::MoveForward),
                ClientKeyBindingConfig::new(
                    ClientKeyCode::KeyS,
                    ClientMovementAction::MoveBackward,
                ),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientControlSettingsConfig {
    pub mouse_sensitivity: f32,
}

impl Default for ClientControlSettingsConfig {
    fn default() -> Self {
        Self {
            mouse_sensitivity: DEFAULT_MOUSE_SENSITIVITY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientCameraSettingsConfig {
    pub move_speed: f32,
    pub voxel_raycast_distance: f32,
}

impl Default for ClientCameraSettingsConfig {
    fn default() -> Self {
        Self {
            move_speed: DEFAULT_CAMERA_MOVE_SPEED,
            voxel_raycast_distance: DEFAULT_VOXEL_RAYCAST_DISTANCE,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientKeyBindingConfig {
    pub key: ClientKeyCode,
    pub actions: Vec<ClientMovementAction>,
}

impl ClientKeyBindingConfig {
    pub fn new(key: ClientKeyCode, action: ClientMovementAction) -> Self {
        Self {
            key,
            actions: vec![action],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientMovementAction {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    MoveForward,
    MoveBackward,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKeyCode {
    Escape,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Backspace,
    Tab,
    KeyQ,
    KeyW,
    KeyE,
    KeyR,
    KeyT,
    KeyY,
    KeyU,
    KeyI,
    KeyO,
    KeyP,
    CapsLock,
    KeyA,
    KeyS,
    KeyD,
    KeyF,
    KeyG,
    KeyH,
    KeyJ,
    KeyK,
    KeyL,
    Enter,
    ShiftLeft,
    KeyZ,
    KeyX,
    KeyC,
    KeyV,
    KeyB,
    KeyN,
    KeyM,
    ShiftRight,
    ControlLeft,
    AltLeft,
    Space,
    AltRight,
    ControlRight,
    ArrowLeft,
    ArrowUp,
    ArrowDown,
    ArrowRight,
}

fn finite_clamped(value: f32, default: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_every_numeric_client_setting() {
        let mut settings = ClientSettingsConfig::default();
        settings.controls.mouse_sensitivity = f32::NAN;
        settings.camera.move_speed = -10.0;
        settings.camera.voxel_raycast_distance = 100.0;

        settings.normalize();

        assert_eq!(
            settings.controls.mouse_sensitivity,
            DEFAULT_MOUSE_SENSITIVITY
        );
        assert_eq!(settings.camera.move_speed, MIN_CAMERA_MOVE_SPEED);
        assert_eq!(
            settings.camera.voxel_raycast_distance,
            MAX_VOXEL_RAYCAST_DISTANCE
        );
    }
}
