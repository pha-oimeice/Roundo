use bevy::{
    feathers::controls::ButtonVariant,
    picking::{cursor::EntityCursor, hover::Hovered},
    prelude::*,
    ui::{InteractionDisabled, Pressed, Selected},
    window::SystemCursorIcon,
};

use super::SIDEBAR;

const HOVER_BACKGROUND: Color = Color::srgb_u8(40, 55, 73);
const SELECTED_BACKGROUND: Color = Color::srgb_u8(59, 79, 102);

#[derive(Clone, Component)]
#[require(Hovered, BackgroundColor)]
pub(super) struct Highlight {
    pub(super) idle: Color,
    pub(super) selected: bool,
}

impl Default for Highlight {
    fn default() -> Self {
        Self {
            idle: SIDEBAR,
            selected: false,
        }
    }
}

pub(super) fn set_button_enabled(
    commands: &mut Commands,
    button: Entity,
    disabled: bool,
    enabled: bool,
) {
    if enabled && disabled {
        commands.entity(button).remove::<InteractionDisabled>();
    } else if !enabled && !disabled {
        commands
            .entity(button)
            .remove::<Pressed>()
            .insert(InteractionDisabled);
    }
}

pub(super) fn preserve_button_cursor(
    event: On<Insert<EntityCursor>>,
    mut cursors: Query<&mut EntityCursor, (With<ButtonVariant>, With<InteractionDisabled>)>,
) {
    // Feathers uses NotAllowed for disabled buttons. Keep its normal button cursor instead.
    if let Ok(mut cursor) = cursors.get_mut(event.entity)
        && *cursor == EntityCursor::System(SystemCursorIcon::NotAllowed)
    {
        *cursor = EntityCursor::System(SystemCursorIcon::Pointer);
    }
}

pub(super) fn paint_highlights(
    mut surfaces: Query<(
        &Highlight,
        &Hovered,
        Has<Selected>,
        Has<InteractionDisabled>,
        &mut BackgroundColor,
    )>,
) {
    for (highlight, hovered, selected, disabled, mut background) in &mut surfaces {
        let color = if disabled {
            highlight.idle
        } else if highlight.selected || selected {
            SELECTED_BACKGROUND
        } else if hovered.get() {
            HOVER_BACKGROUND
        } else {
            highlight.idle
        };
        if background.0 != color {
            background.0 = color;
        }
    }
}
