//! Client snapshots published to the native UI.

use crate::config_store::ClientConfigStore;
use bevy::prelude::*;
use roundo_infrastructure::user_config::{self, ClientInputBindingConfig, ClientInputKey};
use roundo_rendering::DebugCamera;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

pub(crate) const SETTINGS_DATA: &str = "client.settings";
pub(crate) const BINDINGS_DATA: &str = "client.bindings";
pub(crate) const HUD_DATA: &str = "client.hud";
const HUD_SYNC_INTERVAL_SECS: f32 = 0.05;

/// Client diagnostics sampled from the ECS World.
#[derive(Resource, Clone, Debug, Serialize)]
pub(crate) struct HudCache {
    /// Statistics over at most the latest 120 valid frame durations.
    pub fps: HudFps,
    /// Debug camera position, absent if there is not exactly one debug camera.
    pub position: Option<HudPosition>,
    #[serde(skip)]
    frames: VecDeque<f32>,
}

/// Frames-per-second statistics derived from finite positive frame durations.
#[derive(Clone, Debug, Serialize)]
pub struct HudFps {
    pub current: f32,
    pub average: f32,
    pub min: f32,
    pub max: f32,
}
/// Camera position represented as raw world coordinates and floored integer coordinates.
#[derive(Clone, Debug, Serialize)]
pub struct HudPosition {
    pub absolute: [f32; 3],
    /// Each world component is floored and converted to `i32`; this is not a Chunk offset.
    pub chunk_relative: [i32; 3],
}

impl Default for HudCache {
    fn default() -> Self {
        Self {
            fps: HudFps {
                current: 0.0,
                average: 0.0,
                min: 0.0,
                max: 0.0,
            },
            position: None,
            frames: VecDeque::new(),
        }
    }
}

impl HudCache {
    fn record_frame(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        self.frames.push_back(delta_seconds);
        if self.frames.len() > 120 {
            self.frames.pop_front();
        }
        let fps: Vec<f32> = self.frames.iter().map(|delta| delta.recip()).collect();
        self.fps.current = delta_seconds.recip();
        self.fps.average = fps.iter().sum::<f32>() / fps.len() as f32;
        self.fps.min = fps.iter().copied().fold(f32::INFINITY, f32::min);
        self.fps.max = fps.iter().copied().fold(0.0, f32::max);
    }
}

#[derive(Serialize)]
pub(crate) struct BindingDisplay {
    pub(crate) slot: String,
    pub(crate) key: String,
}
#[derive(Serialize)]
pub(crate) struct SupportedKeyDisplay {
    pub(crate) key: String,
    pub(crate) display_name: String,
}
#[derive(Serialize)]
pub(crate) struct SupportedSlotDisplay {
    pub(crate) slot: String,
    pub(crate) display_name: String,
    pub(crate) functions: Vec<String>,
}
#[derive(Serialize)]
pub(crate) struct BindingsSnapshot {
    pub(crate) bindings: Vec<BindingDisplay>,
    pub(crate) supported_keys: Vec<SupportedKeyDisplay>,
    pub(crate) supported_slots: Vec<SupportedSlotDisplay>,
}
pub(crate) fn key_display(key: ClientInputKey) -> SupportedKeyDisplay {
    use ClientInputKey::*;
    let (key, display_name) = match key {
        Digit0 => ("digit0", "0"),
        Digit1 => ("digit1", "1"),
        Digit2 => ("digit2", "2"),
        Digit3 => ("digit3", "3"),
        Digit4 => ("digit4", "4"),
        Digit5 => ("digit5", "5"),
        Digit6 => ("digit6", "6"),
        Digit7 => ("digit7", "7"),
        Digit8 => ("digit8", "8"),
        Digit9 => ("digit9", "9"),
        Backspace => ("backspace", "Backspace"),
        Backquote => ("backquote", "~ / `"),
        Tab => ("tab", "Tab"),
        KeyQ => ("key_q", "Q"),
        KeyW => ("key_w", "W"),
        KeyE => ("key_e", "E"),
        KeyR => ("key_r", "R"),
        KeyT => ("key_t", "T"),
        KeyY => ("key_y", "Y"),
        KeyU => ("key_u", "U"),
        KeyI => ("key_i", "I"),
        KeyO => ("key_o", "O"),
        KeyP => ("key_p", "P"),
        CapsLock => ("caps_lock", "Caps Lock"),
        KeyA => ("key_a", "A"),
        KeyS => ("key_s", "S"),
        KeyD => ("key_d", "D"),
        KeyF => ("key_f", "F"),
        KeyG => ("key_g", "G"),
        KeyH => ("key_h", "H"),
        KeyJ => ("key_j", "J"),
        KeyK => ("key_k", "K"),
        KeyL => ("key_l", "L"),
        Enter => ("enter", "Enter"),
        ShiftLeft => ("shift_left", "Left Shift"),
        KeyZ => ("key_z", "Z"),
        KeyX => ("key_x", "X"),
        KeyC => ("key_c", "C"),
        KeyV => ("key_v", "V"),
        KeyB => ("key_b", "B"),
        KeyN => ("key_n", "N"),
        KeyM => ("key_m", "M"),
        ShiftRight => ("shift_right", "Right Shift"),
        ControlLeft => ("control_left", "Left Control"),
        AltLeft => ("alt_left", "Left Alt"),
        Space => ("space", "Space"),
        AltRight => ("alt_right", "Right Alt"),
        ControlRight => ("control_right", "Right Control"),
        ArrowLeft => ("arrow_left", "Left Arrow"),
        ArrowUp => ("arrow_up", "Up Arrow"),
        ArrowDown => ("arrow_down", "Down Arrow"),
        ArrowRight => ("arrow_right", "Right Arrow"),
        F1 => ("f1", "F1"),
        MouseLeft => ("mouse_left", "Left Mouse Button"),
        MouseRight => ("mouse_right", "Right Mouse Button"),
        MouseMiddle => ("mouse_middle", "Middle Mouse Button"),
        MouseMotion => ("mouse_motion", "Mouse Motion"),
    };
    SupportedKeyDisplay {
        key: key.into(),
        display_name: display_name.into(),
    }
}

pub(crate) fn supported_keys() -> Vec<SupportedKeyDisplay> {
    use ClientInputKey::*;
    [
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
        MouseLeft,
        MouseRight,
        MouseMiddle,
        MouseMotion,
    ]
    .into_iter()
    .map(key_display)
    .collect()
}

pub(crate) fn binding_display(binding: &ClientInputBindingConfig) -> BindingDisplay {
    BindingDisplay {
        slot: binding.slot.clone(),
        key: key_display(binding.key).key,
    }
}

#[derive(Serialize)]
pub(crate) struct SettingDisplay {
    pub(crate) key: String,
    pub(crate) value: f32,
    pub(crate) min: f32,
    pub(crate) max: Option<f32>,
    pub(crate) step: f32,
    pub(crate) default: f32,
}
#[derive(Serialize)]
pub(crate) struct SettingsSnapshot {
    pub(crate) settings: Vec<SettingDisplay>,
}
pub(crate) fn settings_snapshot(config: &ClientConfigStore) -> SettingsSnapshot {
    let settings = config.settings();
    SettingsSnapshot {
        settings: vec![SettingDisplay {
            key: "world.geometry_lod_pixel_error".into(),
            value: settings.world.geometry_lod_pixel_error,
            min: 0.0,
            max: None,
            step: user_config::DEFAULT_GEOMETRY_LOD_PIXEL_ERROR,
            default: user_config::DEFAULT_GEOMETRY_LOD_PIXEL_ERROR,
        }],
    }
}

pub(crate) fn bindings_snapshot(config: &ClientConfigStore) -> BindingsSnapshot {
    BindingsSnapshot {
        bindings: config
            .settings()
            .input_bindings
            .iter()
            .map(binding_display)
            .collect(),
        supported_keys: supported_keys(),
        supported_slots: config
            .settings()
            .input_slots
            .iter()
            .map(|(id, slot)| SupportedSlotDisplay {
                slot: id.clone(),
                display_name: slot.display_name.clone(),
                functions: slot.functions.iter().cloned().collect(),
            })
            .collect(),
    }
}

/// Per-resource accepted snapshots and source-specific publication progress.
#[derive(Resource, Default)]
pub(crate) struct ClientDataSyncState {
    published_generations: BTreeMap<&'static str, u64>,
    hud_elapsed_secs: f32,
}

impl ClientDataSyncState {
    fn publish_snapshot(
        &mut self,
        sink: &mut roundo_ui::UiData,
        resource: &'static str,
        snapshot: serde_json::Value,
    ) -> bool {
        if sink.publish_data(resource, &snapshot) == 0 {
            return false;
        }
        self.published_generations
            .insert(resource, sink.data_subscription_generation());
        true
    }

    fn needs_subscription_snapshot(
        &self,
        sink: &roundo_ui::UiData,
        resource: &'static str,
    ) -> bool {
        let generation = sink.data_subscription_generation();
        self.published_generations.get(resource) != Some(&generation)
    }
}

pub(crate) fn update_hud_cache(
    time: Res<Time>,
    cameras: Query<&Transform, With<DebugCamera>>,
    mut hud: ResMut<HudCache>,
) {
    hud.record_frame(time.delta_secs());
    hud.position = cameras.single().ok().map(|transform| {
        let translation = transform.translation;
        HudPosition {
            absolute: [translation.x, translation.y, translation.z],
            chunk_relative: [
                translation.x.floor() as i32,
                translation.y.floor() as i32,
                translation.z.floor() as i32,
            ],
        }
    });
}

/// Client-owned producers publish directly into the native ECS snapshot store.
/// Live view declarations determine subscriptions; no UI data IPC remains.
pub(crate) fn sync_subscribed_client_data(
    time: Res<Time>,
    config: Res<ClientConfigStore>,
    hud: Res<HudCache>,
    mut sync: ResMut<ClientDataSyncState>,
    mut data: Option<ResMut<roundo_ui::UiData>>,
) {
    let Some(data) = data.as_deref_mut() else {
        return;
    };
    sync_client_data_update(time.delta_secs(), &config, &hud, &mut sync, data);
}

fn sync_client_data_update(
    delta_secs: f32,
    config: &ClientConfigStore,
    hud: &HudCache,
    sync: &mut ClientDataSyncState,
    sink: &mut roundo_ui::UiData,
) {
    if sink.has_data_subscribers(SETTINGS_DATA) {
        let snapshot =
            serde_json::to_value(settings_snapshot(config)).expect("settings snapshot serializes");
        sync.publish_snapshot(sink, SETTINGS_DATA, snapshot);
    }
    if sink.has_data_subscribers(BINDINGS_DATA) {
        let snapshot =
            serde_json::to_value(bindings_snapshot(config)).expect("bindings snapshot serializes");
        sync.publish_snapshot(sink, BINDINGS_DATA, snapshot);
    }

    sync.hud_elapsed_secs += delta_secs;
    if sink.has_data_subscribers(HUD_DATA)
        && (sync.needs_subscription_snapshot(sink, HUD_DATA)
            || sync.hud_elapsed_secs >= HUD_SYNC_INTERVAL_SECS)
    {
        sync.hud_elapsed_secs = 0.0;
        let snapshot = serde_json::to_value(hud).expect("HUD snapshot serializes");
        sync.publish_snapshot(sink, HUD_DATA, snapshot);
    }
}
