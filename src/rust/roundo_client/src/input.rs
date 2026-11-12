//! Sole device-input adapter. Configured bindings produce semantic functions;
//! TOML view actions, not Rust key tables, decide which native UI to open.

use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};
use roundo_marionette::{ClientFunctionFrame, InputFunctionRegistry, MarionetteClientSet};
use roundo_ui::{UiInstanceId, UiLifecycleManager};
use roundo_user_config::ClientInputKey;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum ClientInputSet {
    Collect,
    Gameplay,
}
/// Existing reserved client back/pause intent, independent of editable shortcuts.
#[derive(Message)]
pub struct ClientEscape;
#[derive(Clone, Default)]
struct SlotInputFrame {
    held: BTreeSet<String>,
    pressed: BTreeSet<String>,
    released: BTreeSet<String>,
    motion: BTreeMap<String, Vec2>,
}
#[derive(Default, Resource)]
struct CapturedInput {
    held: BTreeSet<String>,
    blocked: BTreeSet<ClientInputKey>,
    active: bool,
    escape_blocked: bool,
    target: Option<UiInstanceId>,
    frame: SlotInputFrame,
    routes: BTreeMap<String, BTreeSet<String>>,
    bindings: Vec<roundo_user_config::ClientInputBindingConfig>,
}
pub struct ClientInputPlugin;
impl Plugin for ClientInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CapturedInput>()
            .add_message::<ClientEscape>()
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
                collect_input
                    .in_set(ClientInputSet::Collect)
                    .run_if(resource_exists::<roundo_ui::UiReady>),
            )
            .add_systems(
                Update,
                forward_gameplay_input.in_set(ClientInputSet::Gameplay),
            );
    }
}
fn collect_input(world: &mut World) {
    let focused = world
        .query_filtered::<&Window, With<bevy::window::PrimaryWindow>>()
        .single(world)
        .is_ok_and(|w| w.focused);
    let editing = roundo_ui::text_edit_has_focus(world);
    let target = UiLifecycleManager::with(world, |ui| ui.event_target());
    let functions = world
        .run_system_cached_with(collect_frame, (focused, editing, target))
        .expect("input frame system");
    if let Some(target) = target {
        UiLifecycleManager::with(world, |ui| {
            for function in functions {
                ui.dispatch_function(target, &function);
            }
        });
    }
}
fn collect_frame(
    In((focused, editing, target)): In<(bool, bool, Option<UiInstanceId>)>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    config: Res<crate::server_list::ClientConfigStore>,
    registry: Res<InputFunctionRegistry>,
    mut captured: ResMut<CapturedInput>,
    mut escape: MessageWriter<ClientEscape>,
) -> Vec<String> {
    let bindings = &config.settings().input_bindings;
    let routes: BTreeMap<_, _> = config
        .settings()
        .input_slots
        .iter()
        .map(|(id, slot)| (id.clone(), slot.functions.clone()))
        .collect();
    let active = focused && !editing;
    if captured.routes != routes
        || captured.bindings != *bindings
        || target != captured.target
        || (active && !captured.active)
    {
        // A focus/route edit never reinterprets an already-held device input as
        // a new action in its new target. The physical release readmits it.
        let blocked = bindings
            .iter()
            .chain(&captured.bindings)
            .filter(|b| physical(b.key).pressed(&keys, &buttons))
            .map(|b| b.key)
            .collect::<Vec<_>>();
        captured.blocked.extend(blocked);
        captured.bindings = bindings.clone();
    }
    if !focused {
        captured.escape_blocked |= keys.pressed(KeyCode::Escape);
    }
    if !keys.pressed(KeyCode::Escape) && !keys.just_pressed(KeyCode::Escape) {
        captured.escape_blocked = false;
    }
    // Escape remains a host intent while an editor is focused; text widgets do
    // not manufacture a second shortcut path or consume gameplay input.
    if focused && keys.just_pressed(KeyCode::Escape) && !captured.escape_blocked {
        escape.write(ClientEscape);
    }
    captured.blocked.retain(|key| {
        physical(*key).pressed(&keys, &buttons) || physical(*key).just_pressed(&keys, &buttons)
    });
    let mut frame = SlotInputFrame::default();
    if active {
        for binding in bindings {
            if captured.blocked.contains(&binding.key) {
                continue;
            }
            let input = physical(binding.key);
            if input.pressed(&keys, &buttons) {
                frame.held.insert(binding.slot.clone());
            }
            if !captured.held.contains(&binding.slot) && input.just_pressed(&keys, &buttons) {
                frame.pressed.insert(binding.slot.clone());
            }
            if matches!(input, PhysicalInput::Motion) && motion.delta != Vec2::ZERO {
                frame.motion.insert(binding.slot.clone(), motion.delta);
            }
        }
    }
    frame
        .pressed
        .extend(frame.held.difference(&captured.held).cloned());
    frame.released = captured.held.difference(&frame.held).cloned().collect();
    frame
        .released
        .extend(frame.pressed.difference(&frame.held).cloned());
    let functions = frame
        .pressed
        .iter()
        .flat_map(|slot| {
            crate::config::resolve_slot_functions(config.settings(), &registry, slot)
                .expect("validated input route")
                .iter()
                .cloned()
        })
        .collect::<BTreeSet<_>>();
    captured.held = frame.held.clone();
    captured.frame = frame;
    captured.routes = routes;
    captured.target = target;
    captured.active = active;
    functions.into_iter().collect()
}
fn forward_gameplay_input(
    captured: Res<CapturedInput>,
    projection: Res<crate::scene::ClientSceneProjection>,
    config: Res<crate::server_list::ClientConfigStore>,
    registry: Res<InputFunctionRegistry>,
    mut previous: Local<BTreeSet<String>>,
    mut frames: MessageWriter<ClientFunctionFrame>,
) {
    let mut frame = ClientFunctionFrame::default();
    if projection.gameplay_input {
        for slot in &captured.frame.held {
            frame.held.extend(
                crate::config::resolve_slot_functions(config.settings(), &registry, slot)
                    .expect("validated input route")
                    .iter()
                    .cloned(),
            );
        }
        for slot in &captured.frame.pressed {
            for function in
                crate::config::resolve_slot_functions(config.settings(), &registry, slot)
                    .expect("validated input route")
            {
                if !previous.contains(function) {
                    frame.pressed.insert(function.clone());
                }
            }
        }
        for (slot, delta) in &captured.frame.motion {
            for function in
                crate::config::resolve_slot_functions(config.settings(), &registry, slot)
                    .expect("validated input route")
            {
                frame.motion.entry(function.clone()).or_insert(*delta);
            }
        }
    }
    frame
        .pressed
        .extend(frame.held.difference(&previous).cloned());
    frame
        .released
        .extend(previous.difference(&frame.held).cloned());
    frame
        .released
        .extend(frame.pressed.difference(&frame.held).cloned());
    *previous = frame.held.clone();
    frames.write(frame);
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
            Self::Key(k) => keys.pressed(k),
            Self::Mouse(b) => buttons.pressed(b),
            Self::Motion => false,
        }
    }
    fn just_pressed(self, keys: &ButtonInput<KeyCode>, buttons: &ButtonInput<MouseButton>) -> bool {
        match self {
            Self::Key(k) => keys.just_pressed(k),
            Self::Mouse(b) => buttons.just_pressed(b),
            Self::Motion => false,
        }
    }
}
// Device-code translation only, never default/action bindings.
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
