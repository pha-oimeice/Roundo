//! Serializable client controls, camera, world, and input-binding settings.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MIN_MOUSE_SENSITIVITY: f32 = 0.0005;
pub const MAX_MOUSE_SENSITIVITY: f32 = 0.01;
pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 0.002;
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
pub const DEFAULT_GEOMETRY_LOD_PIXEL_ERROR: f32 = 1.0;

/// In-memory aggregate of client categories, not a persisted document format.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientSettingsConfig {
    pub controls: ClientControlSettingsConfig,
    pub camera: ClientCameraSettingsConfig,
    pub world: ClientWorldSettingsConfig,
    /// Client-owned resource identities, independent of editable display names.
    pub input_slots: BTreeMap<String, ClientInputSlotConfig>,
    pub input_categories: Vec<ClientInputCategoryConfig>,
    /// Physical inputs reference existing slots; a slot may have many bindings.
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
        if !self.world.geometry_lod_pixel_error.is_finite()
            || self.world.geometry_lod_pixel_error < 0.0
        {
            self.world.geometry_lod_pixel_error = DEFAULT_GEOMETRY_LOD_PIXEL_ERROR;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientWorldSettingsConfig {
    pub joinable_world_radius: f32,
    pub chunk_view_distance: f32,
    /// Maximum conservative geometric displacement in physical viewport pixels.
    pub geometry_lod_pixel_error: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientControlSettingsConfig {
    pub mouse_sensitivity: f32,
    pub player_movement_prediction: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientCameraSettingsConfig {
    /// Movement speed of the detached client camera only.
    pub move_speed: f32,
    pub voxel_raycast_distance: f32,
}

/// A local shortcut resource. Function references form an unordered many-to-many
/// relation, not an execution sequence or an item-use permission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientInputSlotConfig {
    pub display_name: String,
    pub functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientInputCategoryConfig {
    pub name: String,
    pub actions: Vec<String>,
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
    Backquote,
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
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    PrintScreen,
    ScrollLock,
    Pause,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    SuperLeft,
    SuperRight,
    ContextMenu,
    Insert,
    Home,
    PageUp,
    Delete,
    End,
    PageDown,
    NumLock,
    NumpadDivide,
    NumpadMultiply,
    NumpadSubtract,
    NumpadAdd,
    NumpadEnter,
    NumpadDecimal,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    MouseLeft,
    MouseRight,
    MouseMiddle,
    MouseBack,
    MouseForward,
    MouseMotion,
}

fn finite_clamped(value: f32, default: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        default
    }
}
