use super::UiAction;
use bevy::{prelude::*, ui::InteractionDisabled};

pub(super) const BUTTON_NORMAL: Color = Color::srgb(0.16, 0.18, 0.23);
pub(super) const BUTTON_HOVERED: Color = Color::srgb(0.24, 0.28, 0.36);
pub(super) const BUTTON_PRESSED: Color = Color::srgb(0.18, 0.48, 0.34);
pub(super) const BUTTON_DISABLED: Color = Color::srgb(0.09, 0.1, 0.13);

#[derive(Component)]
pub(super) struct ClientUiRoot;

#[derive(Component, Clone, Copy, Debug)]
pub(super) struct UiButtonAction(pub(super) UiAction);

pub(super) fn spawn_heading(commands: &mut Commands, parent: Entity, value: &str) {
    let entity = commands
        .spawn((
            Text::new(value),
            TextFont {
                font_size: FontSize::Px(34.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                margin: UiRect::bottom(px(10)),
                ..default()
            },
        ))
        .id();
    commands.entity(parent).add_child(entity);
}

pub(super) fn spawn_label(commands: &mut Commands, parent: Entity, value: &str) {
    let entity = commands
        .spawn((
            Text::new(value),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(Color::srgb(0.78, 0.82, 0.9)),
        ))
        .id();
    commands.entity(parent).add_child(entity);
}

pub(super) fn spawn_status(commands: &mut Commands, parent: Entity, value: &str) {
    let entity = commands
        .spawn((
            Text::new(value),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
            TextColor(Color::srgb(0.95, 0.68, 0.35)),
        ))
        .id();
    commands.entity(parent).add_child(entity);
}

pub(super) fn spawn_button(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    action: UiAction,
) -> Entity {
    let button = commands
        .spawn((
            Button,
            UiButtonAction(action),
            Node {
                width: percent(100),
                height: px(46),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: px(1).all(),
                border_radius: BorderRadius::all(px(7)),
                ..default()
            },
            BackgroundColor(BUTTON_NORMAL),
            BorderColor::all(Color::srgb(0.3, 0.34, 0.43)),
        ))
        .id();
    let text = commands
        .spawn((
            Text::new(label),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    commands.entity(button).add_child(text);
    commands.entity(parent).add_child(button);
    button
}

pub(super) fn spawn_adjustment_buttons(
    commands: &mut Commands,
    parent: Entity,
    decrease: UiAction,
    increase: UiAction,
) {
    let row = commands
        .spawn(Node {
            width: percent(100),
            column_gap: px(12),
            ..default()
        })
        .id();
    commands.entity(parent).add_child(row);
    spawn_button(commands, row, "-", decrease);
    spawn_button(commands, row, "+", increase);
}

pub(super) fn spawn_menu_button(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    action: UiAction,
    enabled: bool,
) -> Entity {
    let button = commands
        .spawn((
            Button,
            UiButtonAction(action),
            Node {
                flex_basis: px(0),
                flex_grow: 1.0,
                height: px(42),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: px(1).all(),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(if enabled {
                BUTTON_NORMAL
            } else {
                BUTTON_DISABLED
            }),
            BorderColor::all(if enabled {
                Color::srgb(0.3, 0.34, 0.43)
            } else {
                Color::srgb(0.16, 0.17, 0.2)
            }),
        ))
        .id();
    if !enabled {
        commands.entity(button).insert(InteractionDisabled);
    }
    let text = commands
        .spawn((
            Text::new(label),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
            TextColor(if enabled {
                Color::WHITE
            } else {
                Color::srgb(0.38, 0.4, 0.45)
            }),
        ))
        .id();
    commands.entity(button).add_child(text);
    commands.entity(parent).add_child(button);
    button
}

pub(super) fn spawn_text_input(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    initial_value: &str,
    marker: impl Component,
) {
    use bevy::text::{EditableText, TextCursorStyle};

    spawn_label(commands, parent, label);
    let input = commands
        .spawn((
            marker,
            EditableText::new(initial_value),
            TextCursorStyle::default(),
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                width: percent(100),
                min_height: px(42),
                padding: UiRect::axes(px(10), px(8)),
                border: px(1).all(),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.04, 0.055)),
            BorderColor::all(Color::srgb(0.35, 0.4, 0.52)),
        ))
        .id();
    commands.entity(parent).add_child(input);
}
