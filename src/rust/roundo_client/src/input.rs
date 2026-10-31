//! The client's sole hardware-input boundary. Device adapters resolve registered
//! bindings and emit Bevy messages; no Mod UI action is implemented here.

use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};
use roundo_marionette::{ClientInputFrame, InputRegistry, MarionetteClientSet};
use roundo_user_config::ClientInputKey;
use roundo_webui::{
    UiHostDocumentScript, UiInstanceId, UiLifecycleManager, UiNavigationExecutor, WebUiSystemSet,
};
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum ClientInputSet {
    Collect,
    Gameplay,
}

/// Reserved client back/pause intent. Only this module recognizes its hardware key.
#[derive(Message)]
pub struct ClientEscape;

#[derive(Message)]
struct UiInputAction {
    target: UiInstanceId,
    slot: String,
    value: InputActionValue,
}

#[derive(serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum InputActionValue {
    Pressed,
    Released,
    Motion { delta: [f32; 2] },
}

impl InputActionValue {
    fn button(pressed: bool) -> Self {
        if pressed {
            Self::Pressed
        } else {
            Self::Released
        }
    }
}

#[derive(Default, Resource)]
struct CapturedInput {
    held: BTreeSet<String>,
    blocked: BTreeSet<ClientInputKey>,
    host_active: bool,
    escape_blocked: bool,
    frame: ClientInputFrame,
}

pub struct ClientInputPlugin;
impl Plugin for ClientInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CapturedInput>()
            .add_message::<ClientEscape>()
            .add_message::<UiInputAction>()
            .insert_resource(UiHostDocumentScript(
                include_str!("input/document.js").into(),
            ))
            .configure_sets(
                Update,
                (
                    ClientInputSet::Collect
                        .after(crate::commands::ClientCommandSystemSet::ExecuteAndPublish)
                        .before(crate::scene::ClientSceneSystemSet::Requests),
                    ClientInputSet::Gameplay
                        .after(crate::scene::ClientSceneSystemSet::Project)
                        .before(MarionetteClientSet::Input),
                ),
            )
            .add_systems(
                Update,
                (collect_input, forward_ui_input)
                    .chain()
                    .in_set(ClientInputSet::Collect),
            )
            .add_systems(
                Update,
                forward_gameplay_input.in_set(ClientInputSet::Gameplay),
            )
            .add_systems(
                Update,
                clear_background_focus
                    .after(WebUiSystemSet::Platform)
                    .before(crate::scene::ClientSceneSystemSet::Project),
            );
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DocumentInput {
    Key { code: String, pressed: bool },
    Button { button: u16, pressed: bool },
    Motion { x: f32, y: f32 },
}

fn collect_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    config: Res<crate::server_list::ClientConfigStore>,
    registry: Res<InputRegistry>,
    ui: Res<UiLifecycleManager>,
    mut navigation: NonSendMut<UiNavigationExecutor>,
    mut captured: ResMut<CapturedInput>,
    mut escape: MessageWriter<ClientEscape>,
    mut ui_actions: MessageWriter<UiInputAction>,
) {
    let bindings = crate::config::effective_input_bindings(config.settings(), Some(&registry))
        .expect("input defaults and user bindings were validated before startup");
    let host_active =
        windows.iter().any(|window| window.focused) && navigation.host_has_keyboard_focus();
    let mut frame = ClientInputFrame::default();
    if host_active {
        if !captured.host_active {
            // Never reinterpret a held document shortcut as a fresh host press
            // when closing that document transfers native keyboard focus.
            captured.blocked.extend(
                bindings
                    .iter()
                    .filter(|binding| physical(binding.key).pressed(&keys, &buttons))
                    .map(|binding| binding.key),
            );
            captured.escape_blocked |= keys.pressed(KeyCode::Escape);
        }
        captured.blocked.retain(|key| {
            physical(*key).pressed(&keys, &buttons) || physical(*key).just_pressed(&keys, &buttons)
        });
        if !keys.pressed(KeyCode::Escape) && !keys.just_pressed(KeyCode::Escape) {
            captured.escape_blocked = false;
        }
        for binding in &bindings {
            if captured.blocked.contains(&binding.key) {
                continue;
            }
            if physical(binding.key).pressed(&keys, &buttons) {
                frame.held.insert(binding.slot.clone());
            }
            if !captured.held.contains(&binding.slot)
                && physical(binding.key).just_pressed(&keys, &buttons)
            {
                frame.pressed.insert(binding.slot.clone());
            }
            if matches!(physical(binding.key), PhysicalInput::Motion) && motion.delta != Vec2::ZERO
            {
                frame.motion.insert(binding.slot.clone(), motion.delta);
            }
        }
        if keys.just_pressed(KeyCode::Escape) && !captured.escape_blocked {
            escape.write(ClientEscape);
        }
    }
    captured.host_active = host_active;
    frame
        .pressed
        .extend(frame.held.difference(&captured.held).cloned());
    frame.released = captured.held.difference(&frame.held).cloned().collect();
    // A complete tap may occur within one Update: retain both edges even when
    // the final physical held state is already empty.
    frame
        .released
        .extend(frame.pressed.difference(&frame.held).cloned());
    let target = ui.event_target();
    if let Some(target) = target {
        for slot in &frame.pressed {
            ui_actions.write(UiInputAction {
                target,
                slot: slot.clone(),
                value: InputActionValue::Pressed,
            });
        }
        for slot in &frame.released {
            ui_actions.write(UiInputAction {
                target,
                slot: slot.clone(),
                value: InputActionValue::Released,
            });
        }
        for (slot, delta) in &frame.motion {
            ui_actions.write(UiInputAction {
                target,
                slot: slot.clone(),
                value: InputActionValue::Motion {
                    delta: delta.to_array(),
                },
            });
        }
    }
    captured.held = frame.held.clone();
    captured.frame = frame;

    for (source, payload) in navigation.take_document_messages() {
        // Native transport already checks exact document proof; input additionally
        // requires the current focus target. Background documents cannot inject it.
        if Some(source) != target || !ui.command_source_is_live(source) {
            continue;
        }
        let Ok(input) = serde_json::from_value::<DocumentInput>(payload) else {
            continue;
        };
        if let DocumentInput::Key {
            code,
            pressed: true,
        } = &input
            && code == "Escape"
        {
            captured.escape_blocked = true;
            escape.write(ClientEscape);
            continue;
        }
        if let DocumentInput::Motion { x, y } = &input {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            for binding in &bindings {
                if matches!(physical(binding.key), PhysicalInput::Motion) {
                    ui_actions.write(UiInputAction {
                        target: source,
                        slot: binding.slot.clone(),
                        value: InputActionValue::Motion { delta: [*x, *y] },
                    });
                }
            }
            continue;
        }
        for binding in &bindings {
            let matched = match &input {
                DocumentInput::Key { code, pressed } => (physical(binding.key).code().as_deref()
                    == Some(code.as_str()))
                .then_some(*pressed),
                DocumentInput::Button { button, pressed } => {
                    (physical(binding.key).button() == Some(*button)).then_some(*pressed)
                }
                DocumentInput::Motion { .. } => unreachable!("motion was handled above"),
            };
            if let Some(pressed) = matched {
                if pressed {
                    captured.blocked.insert(binding.key);
                } else {
                    captured.blocked.remove(&binding.key);
                }
                ui_actions.write(UiInputAction {
                    target: source,
                    slot: binding.slot.clone(),
                    value: InputActionValue::button(pressed),
                });
            }
        }
    }
}

/// The only adapter between semantic Bevy messages and Mod document events.
fn forward_ui_input(
    mut actions: MessageReader<UiInputAction>,
    mut navigation: NonSendMut<UiNavigationExecutor>,
) {
    let mut batches = std::collections::BTreeMap::<UiInstanceId, Vec<serde_json::Value>>::new();
    for action in actions.read() {
        let mut payload = serde_json::to_value(&action.value).expect("semantic input serializes");
        payload["slot"] = serde_json::Value::String(action.slot.clone());
        batches.entry(action.target).or_default().push(payload);
    }
    for (target, actions) in batches {
        navigation.publish_event(target, "client.input", &serde_json::Value::Array(actions));
    }
}

fn forward_gameplay_input(
    captured: Res<CapturedInput>,
    projection: Res<crate::scene::ClientSceneProjection>,
    mut frames: MessageWriter<ClientInputFrame>,
) {
    // Always publish, including empty frames: suppressed input must clear held
    // intent instead of being replayed when a UI closes or focus returns.
    frames.write(if projection.gameplay_input {
        captured.frame.clone()
    } else {
        ClientInputFrame::default()
    });
}

fn clear_background_focus(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut ui: ResMut<UiLifecycleManager>,
) {
    if buttons.get_just_pressed().next().is_none() {
        return;
    }
    let Some(position) = windows.iter().next().and_then(Window::cursor_position) else {
        return;
    };
    if ui.interactive_instance_at(position.x, position.y).is_none() {
        ui.clear_focus();
    }
}

#[derive(Clone, Copy)]
enum PhysicalInput {
    Key(KeyCode),
    Mouse(MouseButton),
    Motion,
}
impl PhysicalInput {
    fn pressed(self, keys: &ButtonInput<KeyCode>, buttons: &ButtonInput<MouseButton>) -> bool {
        match self {
            Self::Key(key) => keys.pressed(key),
            Self::Mouse(button) => buttons.pressed(button),
            Self::Motion => false,
        }
    }
    fn just_pressed(self, keys: &ButtonInput<KeyCode>, buttons: &ButtonInput<MouseButton>) -> bool {
        match self {
            Self::Key(key) => keys.just_pressed(key),
            Self::Mouse(button) => buttons.just_pressed(button),
            Self::Motion => false,
        }
    }
    fn code(self) -> Option<String> {
        match self {
            Self::Key(key) => Some(format!("{key:?}")),
            _ => None,
        }
    }
    fn button(self) -> Option<u16> {
        match self {
            Self::Mouse(MouseButton::Left) => Some(0),
            Self::Mouse(MouseButton::Middle) => Some(1),
            Self::Mouse(MouseButton::Right) => Some(2),
            _ => None,
        }
    }
}

// This is a device-code translation table, not a table of action/default bindings.
fn physical(key: ClientInputKey) -> PhysicalInput {
    use ClientInputKey::*;
    PhysicalInput::Key(match key {
        MouseLeft => return PhysicalInput::Mouse(MouseButton::Left),
        MouseRight => return PhysicalInput::Mouse(MouseButton::Right),
        MouseMiddle => return PhysicalInput::Mouse(MouseButton::Middle),
        MouseMotion => return PhysicalInput::Motion,
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
        Backquote => KeyCode::Backquote,
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
    })
}
