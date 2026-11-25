use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use roundo_infrastructure::user_config::{ClientInputKey, ClientSettingsConfig};
use roundo_marionette::{
    LOOK_FUNCTION, MOVE_BACKWARD_FUNCTION, MOVE_DOWN_FUNCTION, MOVE_FORWARD_FUNCTION,
    MOVE_LEFT_FUNCTION, MOVE_RIGHT_FUNCTION, MOVE_UP_FUNCTION,
};

#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Function {
    Left,
    Right,
    Forward,
    Backward,
    Jump,
    Look,
    DetachedDown,
}

impl Function {
    fn resolve(name: &str) -> Result<Self, String> {
        match name {
            MOVE_LEFT_FUNCTION => Ok(Self::Left),
            MOVE_RIGHT_FUNCTION => Ok(Self::Right),
            MOVE_FORWARD_FUNCTION => Ok(Self::Forward),
            MOVE_BACKWARD_FUNCTION => Ok(Self::Backward),
            MOVE_UP_FUNCTION => Ok(Self::Jump),
            LOOK_FUNCTION => Ok(Self::Look),
            MOVE_DOWN_FUNCTION => Ok(Self::DetachedDown),
            _ => Err(format!("Unknown local input function `{name}`")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Left => "Move Left",
            Self::Right => "Move Right",
            Self::Forward => "Move Forward",
            Self::Backward => "Move Backward",
            Self::Jump => "Jump",
            Self::Look => "Look",
            Self::DetachedDown => "Descend (detached camera only)",
        }
    }
}

#[derive(Resource)]
pub(super) struct LocalBindings {
    slots: BTreeMap<String, Slot>,
    actions: BTreeMap<Function, Vec<Binding>>,
}

struct Slot {
    name: String,
    functions: BTreeSet<Function>,
    bindings: Vec<Binding>,
}

impl LocalBindings {
    pub(super) fn resolve(settings: &ClientSettingsConfig) -> Result<Self, String> {
        let mut slots = BTreeMap::new();
        for (id, slot) in &settings.input_slots {
            let functions = slot
                .functions
                .iter()
                .map(|function| Function::resolve(function))
                .collect::<Result<_, _>>()?;
            slots.insert(
                id.clone(),
                Slot {
                    name: slot.display_name.clone(),
                    functions,
                    bindings: Vec::new(),
                },
            );
        }

        for edge in &settings.input_bindings {
            let slot = slots
                .get_mut(&edge.slot)
                .ok_or_else(|| format!("Input binding references unknown slot `{}`", edge.slot))?;
            let binding = physical_binding(edge.key);
            for &function in &slot.functions {
                if (function == Function::Look) != (edge.key == ClientInputKey::MouseMotion) {
                    return Err(format!(
                        "Input `{binding}` in slot `{}` is not valid for function `{}`",
                        edge.slot,
                        function.label(),
                    ));
                }
            }
            if !slot.bindings.contains(&binding) {
                slot.bindings.push(binding);
            }
        }

        let mut actions: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for slot in slots.values() {
            for &function in &slot.functions {
                if function == Function::DetachedDown {
                    continue;
                }
                let bindings = actions.entry(function).or_default();
                for &binding in &slot.bindings {
                    if !bindings.contains(&binding) {
                        bindings.push(binding);
                    }
                }
            }
        }
        Ok(Self { slots, actions })
    }

    pub(super) fn control_rows(&self) -> impl Iterator<Item = String> + '_ {
        self.slots.values().map(|slot| {
            let functions = slot
                .functions
                .iter()
                .map(|function| function.label())
                .collect::<Vec<_>>()
                .join(" / ");
            let keys = slot
                .bindings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" / ");
            let functions = if functions.is_empty() {
                "No function assigned"
            } else {
                &functions
            };
            let keys = if keys.is_empty() { "Unbound" } else { &keys };
            format!("{} · {functions}: {keys}", slot.name)
        })
    }

    pub(super) fn attach(&self, commands: &mut Commands, character: Entity) {
        for (&function, bindings) in &self.actions {
            let action = match function {
                Function::Look => commands
                    .spawn((
                        Action::<LookInput>::new(),
                        ActionOf::<LocalCharacter>::new(character),
                    ))
                    .id(),
                _ => commands
                    .spawn((
                        function,
                        Action::<DigitalInput>::new(),
                        ActionOf::<LocalCharacter>::new(character),
                    ))
                    .id(),
            };
            for &binding in bindings {
                commands.spawn((binding, BindingOf(action)));
            }
        }
    }
}

#[derive(Component)]
pub(super) struct LocalCharacter;

#[derive(InputAction)]
#[action_output(bool)]
pub(super) struct DigitalInput;

#[derive(InputAction)]
#[action_output(Vec2)]
pub(super) struct LookInput;

#[derive(Component, Default)]
pub(super) struct LocalIntent {
    movement: Vec3,
    pub(super) pending_look: Vec2,
    jump_held: bool,
    jump_pending: bool,
    capturing: bool,
}

impl LocalIntent {
    pub(super) fn take_movement(&mut self) -> Vec3 {
        // A tap can start and end between two fixed ticks.
        let jump = std::mem::take(&mut self.jump_pending) || self.jump_held;
        self.movement.with_y(f32::from(jump)).clamp_length_max(1.0)
    }
}

pub(super) fn sample_actions(
    character: Option<
        Single<(
            &ContextActivity<LocalCharacter>,
            Option<&Actions<LocalCharacter>>,
            &mut LocalIntent,
        )>,
    >,
    digital: Query<(&Function, &Action<DigitalInput>)>,
    look: Query<&Action<LookInput>>,
) {
    let Some(character) = character else { return };
    let (active, actions, mut intent) = character.into_inner();
    if !**active {
        *intent = LocalIntent::default();
        return;
    }
    let mut movement = Vec3::ZERO;
    let mut jump = false;
    if let Some(actions) = actions {
        for (function, action) in digital.iter_many(actions).matched() {
            let strength = f32::from(**action);
            match function {
                Function::Left => movement.x -= strength,
                Function::Right => movement.x += strength,
                Function::Forward => movement.z += strength,
                Function::Backward => movement.z -= strength,
                Function::Jump => jump = **action,
                Function::Look | Function::DetachedDown => unreachable!("not a digital action"),
            }
        }
        // Ignore mouse movement sampled while transitioning from menu to capture.
        if intent.capturing {
            for action in look.iter_many(actions).matched() {
                intent.pending_look += **action;
            }
        }
    }
    intent.movement = movement;
    intent.jump_pending |= jump && !intent.jump_held;
    intent.jump_held = jump;
    intent.capturing = true;
}

fn physical_binding(key: ClientInputKey) -> Binding {
    use ClientInputKey::*;
    match key {
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
        MouseLeft => return MouseButton::Left.into(),
        MouseRight => return MouseButton::Right.into(),
        MouseMiddle => return MouseButton::Middle.into(),
        MouseMotion => return Binding::mouse_motion(),
    }
    .into()
}
