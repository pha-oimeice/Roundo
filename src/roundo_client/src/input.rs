//! Sole device-input adapter. Configured bindings produce semantic functions;
//! TOML views can map these functions to local view actions.

use bevy::prelude::*;
use roundo_marionette::InputFunctionRegistry;
use roundo_ui::{UiInstanceId, UiLifecycleManager, UiReady, UiSystemSet};
use roundo_user_config::ClientInputKey;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Resource)]
struct CapturedInput {
    held: BTreeSet<String>,
    blocked: BTreeSet<ClientInputKey>,
    active: bool,
    escape_blocked: bool,
    target: Option<UiInstanceId>,
    routes: BTreeMap<String, BTreeSet<String>>,
    bindings: Vec<roundo_user_config::ClientInputBindingConfig>,
}
pub struct ClientInputPlugin;
impl Plugin for ClientInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CapturedInput>().add_systems(
            Update,
            collect_input
                .after(UiSystemSet::Ingress)
                .before(UiSystemSet::Project)
                .run_if(resource_exists::<UiReady>),
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
    let (functions, escape) = world
        .run_system_cached_with(collect_frame, (focused, editing, target))
        .expect("input frame system");
    UiLifecycleManager::with(world, |ui| {
        if let Some(target) = target {
            for function in functions {
                ui.dispatch_function(target, &function);
            }
        }
        if escape
            && let Some(focused) = ui.focused_instance()
            && !ui.is_root(focused)
            && let Err(error) = ui.back(focused)
        {
            log::warn!("native UI back rejected: {error}");
        }
    });
}
fn collect_frame(
    In((focused, editing, target)): In<(bool, bool, Option<UiInstanceId>)>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    config: Res<crate::config_store::ClientConfigStore>,
    registry: Res<InputFunctionRegistry>,
    mut captured: ResMut<CapturedInput>,
) -> (BTreeSet<String>, bool) {
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
    // Escape remains a host back action while an editor is focused.
    let escape = focused && keys.just_pressed(KeyCode::Escape) && !captured.escape_blocked;
    captured.blocked.retain(|key| {
        physical(*key).pressed(&keys, &buttons) || physical(*key).just_pressed(&keys, &buttons)
    });
    let mut held = BTreeSet::new();
    let mut pressed = BTreeSet::new();
    if active {
        for binding in bindings {
            if captured.blocked.contains(&binding.key) {
                continue;
            }
            let input = physical(binding.key);
            if input.pressed(&keys, &buttons) {
                held.insert(binding.slot.clone());
            }
            if !captured.held.contains(&binding.slot) && input.just_pressed(&keys, &buttons) {
                pressed.insert(binding.slot.clone());
            }
        }
    }
    pressed.extend(held.difference(&captured.held).cloned());
    let functions = pressed
        .iter()
        .flat_map(|slot| {
            crate::config::resolve_slot_functions(config.settings(), &registry, slot)
                .expect("validated input route")
                .iter()
                .cloned()
        })
        .collect();
    captured.held = held;
    captured.routes = routes;
    captured.target = target;
    captured.active = active;
    (functions, escape)
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
