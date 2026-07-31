use super::{
    UiAction,
    fonts::UiFontRole,
    s4_settings::SettingsUiState,
    state::ClientUiState,
    widgets::{
        BUTTON_PRESSED, UiButtonPalette, spawn_compact_button, spawn_compact_symbol_button,
        spawn_label,
    },
};
use bevy::{ecs::relationship::Relationship, picking::Pickable, prelude::*};
use roundo_marionette::{ClientKeyBindings, MovementAction};

const ACTION_PANEL_BACKGROUND: Color = Color::srgb(0.07, 0.083, 0.112);
const KEYBOARD_BACKGROUND: Color = Color::srgb(0.045, 0.055, 0.075);
const BOUND_KEY_NORMAL: Color = Color::srgb(0.16, 0.42, 0.34);
const BOUND_KEY_HOVERED: Color = Color::srgb(0.2, 0.54, 0.43);
const QUEUE_ROW_NORMAL: Color = Color::srgb(0.09, 0.11, 0.15);
const QUEUE_ROW_DRAGGING: Color = Color::srgb(0.17, 0.38, 0.32);
const EXPANDED_PREFIX: &str = "−";
const REMOVE_SYMBOL: &str = "×";
const DRAG_HANDLE: &str = "☰";

#[derive(Clone, Copy)]
struct KeyCap {
    code: KeyCode,
    label: &'static str,
    width: f32,
    symbol: bool,
}

impl KeyCap {
    const fn new(code: KeyCode, label: &'static str, width: f32) -> Self {
        Self {
            code,
            label,
            width,
            symbol: false,
        }
    }

    const fn symbol(code: KeyCode, label: &'static str, width: f32) -> Self {
        Self {
            code,
            label,
            width,
            symbol: true,
        }
    }
}

const NUMBER_ROW: [KeyCap; 12] = [
    KeyCap::new(KeyCode::Escape, "Esc", 52.0),
    KeyCap::new(KeyCode::Digit1, "1", 34.0),
    KeyCap::new(KeyCode::Digit2, "2", 34.0),
    KeyCap::new(KeyCode::Digit3, "3", 34.0),
    KeyCap::new(KeyCode::Digit4, "4", 34.0),
    KeyCap::new(KeyCode::Digit5, "5", 34.0),
    KeyCap::new(KeyCode::Digit6, "6", 34.0),
    KeyCap::new(KeyCode::Digit7, "7", 34.0),
    KeyCap::new(KeyCode::Digit8, "8", 34.0),
    KeyCap::new(KeyCode::Digit9, "9", 34.0),
    KeyCap::new(KeyCode::Digit0, "0", 34.0),
    KeyCap::new(KeyCode::Backspace, "Back", 70.0),
];

const TOP_ROW: [KeyCap; 11] = [
    KeyCap::new(KeyCode::Tab, "Tab", 52.0),
    KeyCap::new(KeyCode::KeyQ, "Q", 34.0),
    KeyCap::new(KeyCode::KeyW, "W", 34.0),
    KeyCap::new(KeyCode::KeyE, "E", 34.0),
    KeyCap::new(KeyCode::KeyR, "R", 34.0),
    KeyCap::new(KeyCode::KeyT, "T", 34.0),
    KeyCap::new(KeyCode::KeyY, "Y", 34.0),
    KeyCap::new(KeyCode::KeyU, "U", 34.0),
    KeyCap::new(KeyCode::KeyI, "I", 34.0),
    KeyCap::new(KeyCode::KeyO, "O", 34.0),
    KeyCap::new(KeyCode::KeyP, "P", 34.0),
];

const HOME_ROW: [KeyCap; 11] = [
    KeyCap::new(KeyCode::CapsLock, "Caps", 64.0),
    KeyCap::new(KeyCode::KeyA, "A", 34.0),
    KeyCap::new(KeyCode::KeyS, "S", 34.0),
    KeyCap::new(KeyCode::KeyD, "D", 34.0),
    KeyCap::new(KeyCode::KeyF, "F", 34.0),
    KeyCap::new(KeyCode::KeyG, "G", 34.0),
    KeyCap::new(KeyCode::KeyH, "H", 34.0),
    KeyCap::new(KeyCode::KeyJ, "J", 34.0),
    KeyCap::new(KeyCode::KeyK, "K", 34.0),
    KeyCap::new(KeyCode::KeyL, "L", 34.0),
    KeyCap::new(KeyCode::Enter, "Enter", 64.0),
];

const BOTTOM_ROW: [KeyCap; 9] = [
    KeyCap::new(KeyCode::ShiftLeft, "Shift", 80.0),
    KeyCap::new(KeyCode::KeyZ, "Z", 34.0),
    KeyCap::new(KeyCode::KeyX, "X", 34.0),
    KeyCap::new(KeyCode::KeyC, "C", 34.0),
    KeyCap::new(KeyCode::KeyV, "V", 34.0),
    KeyCap::new(KeyCode::KeyB, "B", 34.0),
    KeyCap::new(KeyCode::KeyN, "N", 34.0),
    KeyCap::new(KeyCode::KeyM, "M", 34.0),
    KeyCap::new(KeyCode::ShiftRight, "Shift", 80.0),
];

const SPACE_ROW: [KeyCap; 5] = [
    KeyCap::new(KeyCode::ControlLeft, "Ctrl", 54.0),
    KeyCap::new(KeyCode::AltLeft, "Alt", 48.0),
    KeyCap::new(KeyCode::Space, "Space", 200.0),
    KeyCap::new(KeyCode::AltRight, "Alt", 48.0),
    KeyCap::new(KeyCode::ControlRight, "Ctrl", 54.0),
];

const ARROW_ROW: [KeyCap; 4] = [
    KeyCap::symbol(KeyCode::ArrowLeft, "⬅", 52.0),
    KeyCap::symbol(KeyCode::ArrowUp, "⬆", 52.0),
    KeyCap::symbol(KeyCode::ArrowDown, "⬇", 52.0),
    KeyCap::symbol(KeyCode::ArrowRight, "➡", 52.0),
];

const KEYBOARD_ROWS: [&[KeyCap]; 6] = [
    &NUMBER_ROW,
    &TOP_ROW,
    &HOME_ROW,
    &BOTTOM_ROW,
    &SPACE_ROW,
    &ARROW_ROW,
];

#[derive(Component, Clone, Copy)]
struct BindingQueueRow {
    key: KeyCode,
    action: MovementAction,
}

pub(super) fn spawn(
    commands: &mut Commands,
    parent: Entity,
    canvas: Entity,
    bindings: &ClientKeyBindings,
    ui_state: &SettingsUiState,
) {
    let workspace = commands
        .spawn(Node {
            width: percent(100),
            flex_grow: 1.0,
            align_items: AlignItems::Stretch,
            column_gap: px(14),
            ..default()
        })
        .id();
    commands.entity(parent).add_child(workspace);

    spawn_action_panel(commands, workspace, bindings, ui_state);
    spawn_keyboard(commands, workspace, bindings);

    if let Some(key) = ui_state.selected_key() {
        spawn_key_queue_popup(commands, canvas, key, bindings);
    }
}

fn spawn_action_panel(
    commands: &mut Commands,
    parent: Entity,
    bindings: &ClientKeyBindings,
    ui_state: &SettingsUiState,
) {
    let panel = commands
        .spawn((
            Node {
                width: px(300),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(5),
                padding: px(10).all(),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(ACTION_PANEL_BACKGROUND),
            BorderColor::all(Color::srgb(0.18, 0.22, 0.3)),
        ))
        .id();
    commands.entity(parent).add_child(panel);

    spawn_label(commands, panel, "Movement actions");
    spawn_small_text(
        commands,
        panel,
        "Expand an action to inspect every key that triggers it.",
    );

    for action in MovementAction::ALL {
        spawn_action_mapping(commands, panel, action, bindings, ui_state);
    }
}

fn spawn_action_mapping(
    commands: &mut Commands,
    parent: Entity,
    action: MovementAction,
    bindings: &ClientKeyBindings,
    ui_state: &SettingsUiState,
) {
    let expanded = ui_state.movement_action_expanded(action);
    let container = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            ..default()
        })
        .id();
    commands.entity(parent).add_child(container);

    let prefix = if expanded { EXPANDED_PREFIX } else { "+" };
    let keys = bindings.keys_for(action);
    let header = spawn_compact_button(
        commands,
        container,
        &format!("{prefix}  {}  ({})", action.label(), keys.len()),
        UiAction::ToggleMovementAction(action),
        percent(100),
    );
    commands.entity(header).insert(Node {
        width: percent(100),
        height: px(36),
        justify_content: JustifyContent::FlexStart,
        padding: UiRect::horizontal(px(10)),
        border_radius: BorderRadius::all(px(5)),
        ..default()
    });

    if !expanded {
        return;
    }

    let list = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            row_gap: px(4),
            padding: UiRect::new(px(8), px(4), px(4), px(7)),
            ..default()
        })
        .id();
    commands.entity(container).add_child(list);

    if keys.is_empty() {
        spawn_small_text(commands, list, "No keys bound.");
        return;
    }

    for key in keys {
        let queue_position = bindings
            .actions_for(key)
            .iter()
            .position(|bound| *bound == action)
            .map(|index| index + 1)
            .unwrap_or_default();
        let row = commands
            .spawn(Node {
                width: percent(100),
                height: px(32),
                align_items: AlignItems::Center,
                column_gap: px(6),
                ..default()
            })
            .id();
        commands.entity(list).add_child(row);
        spawn_compact_button(
            commands,
            row,
            &key_label(key),
            UiAction::OpenKeyBindingQueue(key),
            px(88),
        );
        spawn_small_text(commands, row, &format!("queue #{queue_position}"));
        spawn_compact_button(
            commands,
            row,
            REMOVE_SYMBOL,
            UiAction::RemoveKeyBinding(key, action),
            px(32),
        );
    }
}

fn spawn_keyboard(commands: &mut Commands, parent: Entity, bindings: &ClientKeyBindings) {
    let panel = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(7),
                padding: px(12).all(),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(KEYBOARD_BACKGROUND),
            BorderColor::all(Color::srgb(0.18, 0.22, 0.3)),
        ))
        .id();
    commands.entity(parent).add_child(panel);

    spawn_label(commands, panel, "Keyboard");
    spawn_small_text(
        commands,
        panel,
        "Select a key to edit its ordered action queue.",
    );

    let keyboard = commands
        .spawn(Node {
            width: percent(100),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: px(6),
            ..default()
        })
        .id();
    commands.entity(panel).add_child(keyboard);

    for key_row in KEYBOARD_ROWS {
        let row = commands
            .spawn(Node {
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                column_gap: px(4),
                ..default()
            })
            .id();
        commands.entity(keyboard).add_child(row);

        for key in key_row {
            let spawn_key_button = if key.symbol {
                spawn_compact_symbol_button
            } else {
                spawn_compact_button
            };
            let button = spawn_key_button(
                commands,
                row,
                key.label,
                UiAction::OpenKeyBindingQueue(key.code),
                px(key.width),
            );
            commands.entity(button).insert(Node {
                width: px(key.width),
                height: px(34),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: px(1).all(),
                border_radius: BorderRadius::all(px(5)),
                ..default()
            });
            if !bindings.actions_for(key.code).is_empty() {
                commands.entity(button).insert((
                    BackgroundColor(BOUND_KEY_NORMAL),
                    UiButtonPalette {
                        normal: BOUND_KEY_NORMAL,
                        hovered: BOUND_KEY_HOVERED,
                        pressed: BUTTON_PRESSED,
                    },
                ));
            }
        }
    }
}

fn spawn_key_queue_popup(
    commands: &mut Commands,
    canvas: Entity,
    key: KeyCode,
    bindings: &ClientKeyBindings,
) {
    let overlay = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.68)),
            GlobalZIndex(40),
        ))
        .id();
    commands.entity(canvas).add_child(overlay);

    let dialog = commands
        .spawn((
            Node {
                width: px(560),
                max_height: percent(86),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(10),
                padding: px(18).all(),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(10)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.055, 0.068, 0.094)),
            BorderColor::all(Color::srgb(0.28, 0.34, 0.44)),
        ))
        .id();
    commands.entity(overlay).add_child(dialog);

    let header = commands
        .spawn(Node {
            width: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        })
        .id();
    commands.entity(dialog).add_child(header);
    let title = commands
        .spawn((
            Text::new(format!("{} action queue", key_label(key))),
            TextFont {
                font_size: FontSize::Px(24.0),
                ..default()
            },
            UiFontRole::Semibold,
            TextColor(Color::WHITE),
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(header).add_child(title);
    spawn_compact_button(
        commands,
        header,
        "Close",
        UiAction::CloseKeyBindingQueue,
        px(82),
    );

    spawn_small_text(
        commands,
        dialog,
        "When this key is held, every action runs in the numbered order. Drag rows to reorder.",
    );

    let queue = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            row_gap: px(6),
            ..default()
        })
        .id();
    commands.entity(dialog).add_child(queue);

    let actions = bindings.actions_for(key);
    if actions.is_empty() {
        spawn_small_text(commands, queue, "This key has no actions.");
    } else {
        for (index, action) in actions.iter().copied().enumerate() {
            spawn_queue_row(commands, queue, key, action, index);
        }
    }

    spawn_label(commands, dialog, "Add action");
    let available = commands
        .spawn(Node {
            width: percent(100),
            flex_wrap: FlexWrap::Wrap,
            column_gap: px(6),
            row_gap: px(6),
            ..default()
        })
        .id();
    commands.entity(dialog).add_child(available);

    let mut available_count = 0;
    for action in MovementAction::ALL {
        if actions.contains(&action) {
            continue;
        }
        available_count += 1;
        spawn_compact_button(
            commands,
            available,
            action.label(),
            UiAction::AddKeyBinding(key, action),
            px(160),
        );
    }
    if available_count == 0 {
        spawn_small_text(
            commands,
            available,
            "All movement actions are already bound.",
        );
    }
}

fn spawn_queue_row(
    commands: &mut Commands,
    parent: Entity,
    key: KeyCode,
    action: MovementAction,
    index: usize,
) {
    let mut row = commands.spawn((
        BindingQueueRow { key, action },
        Pickable {
            should_block_lower: false,
            is_hoverable: true,
        },
        Node {
            width: percent(100),
            height: px(42),
            align_items: AlignItems::Center,
            column_gap: px(10),
            padding: UiRect::horizontal(px(10)),
            border: px(1).all(),
            border_radius: BorderRadius::all(px(6)),
            ..default()
        },
        UiTransform::default(),
        ZIndex::default(),
        BackgroundColor(QUEUE_ROW_NORMAL),
        BorderColor::all(Color::srgb(0.22, 0.27, 0.35)),
    ));
    row.observe(begin_queue_drag)
        .observe(move_queue_drag)
        .observe(end_queue_drag)
        .observe(reorder_binding_queue);
    let row = row.id();
    commands.entity(parent).add_child(row);

    let number = commands
        .spawn((
            Text::new(format!("{}.", index + 1)),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
            TextColor(Color::srgb(0.5, 0.8, 0.68)),
            Pickable::IGNORE,
            Node {
                width: px(28),
                ..default()
            },
        ))
        .id();
    commands.entity(row).add_child(number);

    let handle = commands
        .spawn((
            Text::new(DRAG_HANDLE),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            UiFontRole::Symbols,
            TextColor(Color::srgb(0.55, 0.6, 0.68)),
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(row).add_child(handle);

    let label = commands
        .spawn((
            Text::new(action.label()),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Pickable::IGNORE,
            Node {
                flex_grow: 1.0,
                ..default()
            },
        ))
        .id();
    commands.entity(row).add_child(label);

    spawn_compact_button(
        commands,
        row,
        "Remove",
        UiAction::RemoveKeyBinding(key, action),
        px(82),
    );
}

fn begin_queue_drag(
    mut event: On<Pointer<DragStart>>,
    mut rows: Query<(&mut BackgroundColor, &mut ZIndex), With<BindingQueueRow>>,
) {
    if let Ok((mut background, mut z_index)) = rows.get_mut(event.event_target()) {
        background.0 = QUEUE_ROW_DRAGGING;
        z_index.0 = 1;
        event.propagate(false);
    }
}

fn move_queue_drag(
    mut event: On<Pointer<Drag>>,
    mut rows: Query<&mut UiTransform, With<BindingQueueRow>>,
) {
    if let Ok(mut transform) = rows.get_mut(event.event_target()) {
        transform.translation = Val2::px(0.0, event.distance.y);
        event.propagate(false);
    }
}

fn end_queue_drag(
    mut event: On<Pointer<DragEnd>>,
    mut rows: Query<(&mut UiTransform, &mut BackgroundColor, &mut ZIndex), With<BindingQueueRow>>,
) {
    if let Ok((mut transform, mut background, mut z_index)) = rows.get_mut(event.event_target()) {
        transform.translation = Val2::ZERO;
        background.0 = QUEUE_ROW_NORMAL;
        z_index.0 = 0;
        event.propagate(false);
    }
}

fn reorder_binding_queue(
    mut event: On<Pointer<DragDrop>>,
    rows: Query<&BindingQueueRow>,
    parents: Query<&ChildOf>,
    mut bindings: ResMut<ClientKeyBindings>,
    mut ui_state: ResMut<ClientUiState>,
) {
    let Some(target) = find_queue_row(event.event_target(), &rows, &parents) else {
        return;
    };
    let Some(dropped) = find_queue_row(event.dropped, &rows, &parents) else {
        return;
    };
    if dropped.key == target.key && bindings.reorder(dropped.key, dropped.action, target.action) {
        ui_state.touch();
    }
    event.propagate(false);
}

fn find_queue_row(
    mut entity: Entity,
    rows: &Query<&BindingQueueRow>,
    parents: &Query<&ChildOf>,
) -> Option<BindingQueueRow> {
    loop {
        if let Ok(row) = rows.get(entity) {
            return Some(*row);
        }
        entity = parents.get(entity).ok()?.get();
    }
}

fn spawn_small_text(commands: &mut Commands, parent: Entity, value: &str) {
    let text = commands
        .spawn((
            Text::new(value),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::srgb(0.5, 0.55, 0.64)),
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(parent).add_child(text);
}

pub(super) fn key_label(key: KeyCode) -> String {
    for row in KEYBOARD_ROWS {
        if let Some(key_cap) = row.iter().find(|key_cap| key_cap.code == key) {
            return key_cap.label.to_string();
        }
    }
    format!("{key:?}")
}
