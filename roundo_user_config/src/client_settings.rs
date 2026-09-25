//! Serializable client controls, camera, world, and input-binding settings.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MIN_MOUSE_SENSITIVITY: f32 = 0.0005;
pub const MAX_MOUSE_SENSITIVITY: f32 = 0.01;
pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 0.002;
pub const DEFAULT_PLAYER_MOVEMENT_PREDICTION: bool = false;
pub const MIN_CAMERA_MOVE_SPEED: f32 = 1.0;
pub const MAX_CAMERA_MOVE_SPEED: f32 = 50.0;
pub const DEFAULT_CAMERA_MOVE_SPEED: f32 = 5.0;
pub const MIN_VOXEL_RAYCAST_DISTANCE: f32 = 1.0;
pub const MAX_VOXEL_RAYCAST_DISTANCE: f32 = 16.0;
pub const DEFAULT_VOXEL_RAYCAST_DISTANCE: f32 = 8.0;
pub const MIN_JOINABLE_WORLD_RADIUS: f32 = 0.5;
pub const MAX_JOINABLE_WORLD_RADIUS: f32 = 32.0;
pub const DEFAULT_JOINABLE_WORLD_RADIUS: f32 = 4.0;
pub const MIN_CHUNK_VIEW_DISTANCE: f32 = 1.0;
pub const MAX_CHUNK_VIEW_DISTANCE: f32 = 512.0;
pub const DEFAULT_CHUNK_VIEW_DISTANCE: f32 = 64.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettingsConfig {
    pub controls: ClientControlSettingsConfig,
    pub camera: ClientCameraSettingsConfig,
    pub world: ClientWorldSettingsConfig,
    /// Many-to-many edges between Mod-provided semantic slots and physical inputs.
    pub input_bindings: Vec<ClientInputBindingConfig>,
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
        self.world.joinable_world_radius = finite_clamped(
            self.world.joinable_world_radius,
            DEFAULT_JOINABLE_WORLD_RADIUS,
            MIN_JOINABLE_WORLD_RADIUS,
            MAX_JOINABLE_WORLD_RADIUS,
        );
        self.world.chunk_view_distance = finite_clamped(
            self.world.chunk_view_distance,
            DEFAULT_CHUNK_VIEW_DISTANCE,
            MIN_CHUNK_VIEW_DISTANCE,
            MAX_CHUNK_VIEW_DISTANCE,
        )
        .round();
        let mut seen = BTreeSet::new();
        self.input_bindings
            .retain(|binding| seen.insert((binding.slot.clone(), binding.key)));
        // Existing development configs predate this Mod slot. Preserve any
        // explicit rebind, but backfill the default edge when the slot is absent.
        if !self
            .input_bindings
            .iter()
            .any(|binding| binding.slot == "roundo.spawn-test-creature")
        {
            self.input_bindings.push(ClientInputBindingConfig::new(
                "roundo.spawn-test-creature",
                ClientInputKey::Digit1,
            ));
        }
    }
}

impl Default for ClientSettingsConfig {
    fn default() -> Self {
        Self {
            controls: ClientControlSettingsConfig::default(),
            camera: ClientCameraSettingsConfig::default(),
            world: ClientWorldSettingsConfig::default(),
            input_bindings: vec![
                ClientInputBindingConfig::new("roundo.move-up", ClientInputKey::Space),
                ClientInputBindingConfig::new("roundo.move-down", ClientInputKey::ShiftLeft),
                ClientInputBindingConfig::new("roundo.move-left", ClientInputKey::KeyA),
                ClientInputBindingConfig::new("roundo.move-right", ClientInputKey::KeyD),
                ClientInputBindingConfig::new("roundo.move-forward", ClientInputKey::KeyW),
                ClientInputBindingConfig::new("roundo.move-backward", ClientInputKey::KeyS),
                ClientInputBindingConfig::new("roundo.spirit-camera", ClientInputKey::F1),
                ClientInputBindingConfig::new("roundo.destroy-block", ClientInputKey::MouseLeft),
                ClientInputBindingConfig::new("roundo.place-block", ClientInputKey::MouseRight),
                ClientInputBindingConfig::new("roundo.spawn-test-creature", ClientInputKey::Digit1),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientWorldSettingsConfig {
    pub joinable_world_radius: f32,
    pub chunk_view_distance: f32,
}
impl Default for ClientWorldSettingsConfig {
    fn default() -> Self {
        Self {
            joinable_world_radius: DEFAULT_JOINABLE_WORLD_RADIUS,
            chunk_view_distance: DEFAULT_CHUNK_VIEW_DISTANCE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientControlSettingsConfig {
    pub mouse_sensitivity: f32,
    pub player_movement_prediction: bool,
}
impl Default for ClientControlSettingsConfig {
    fn default() -> Self {
        Self {
            mouse_sensitivity: DEFAULT_MOUSE_SENSITIVITY,
            player_movement_prediction: DEFAULT_PLAYER_MOVEMENT_PREDICTION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientCameraSettingsConfig {
    /// Movement speed of the detached F1 client camera only.
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
pub struct ClientInputBindingConfig {
    pub slot: String,
    pub key: ClientInputKey,
}
impl ClientInputBindingConfig {
    pub fn new(slot: impl Into<String>, key: ClientInputKey) -> Self {
        Self {
            slot: slot.into(),
            key,
        }
    }
}

/// Stable serialized physical inputs. Escape is deliberately absent and cannot
/// be configured because the client host owns it as the pause/back key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientInputKey {
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
    F1,
    MouseLeft,
    MouseRight,
    MouseMiddle,
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
    fn normalizes_numeric_settings_and_duplicate_binding_edges() {
        let mut settings = ClientSettingsConfig::default();
        settings.controls.mouse_sensitivity = f32::NAN;
        settings.camera.move_speed = -10.0;
        settings.camera.voxel_raycast_distance = 100.0;
        settings.world.joinable_world_radius = f32::INFINITY;
        settings.world.chunk_view_distance = 63.6;
        settings
            .input_bindings
            .push(settings.input_bindings[0].clone());
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
        assert_eq!(
            settings.world.joinable_world_radius,
            DEFAULT_JOINABLE_WORLD_RADIUS
        );
        assert_eq!(settings.world.chunk_view_distance, 64.0);
        assert_eq!(settings.input_bindings.len(), 10);
    }

    #[test]
    fn normalization_backfills_spawn_test_creature_for_existing_configs() {
        let mut settings = ClientSettingsConfig::default();
        settings
            .input_bindings
            .retain(|binding| binding.slot != "roundo.spawn-test-creature");
        settings.normalize();
        assert!(settings.input_bindings.iter().any(|binding| {
            binding.slot == "roundo.spawn-test-creature" && binding.key == ClientInputKey::Digit1
        }));
    }

    #[test]
    fn defaults_cover_every_builtin_slot_without_escape() {
        let settings = ClientSettingsConfig::default();
        assert_eq!(settings.input_bindings.len(), 10);
        assert!(
            settings
                .input_bindings
                .iter()
                .any(|binding| binding.key == ClientInputKey::F1)
        );
        assert!(settings.input_bindings.iter().any(|binding| {
            binding.slot == "roundo.spawn-test-creature" && binding.key == ClientInputKey::Digit1
        }));
    }
}
