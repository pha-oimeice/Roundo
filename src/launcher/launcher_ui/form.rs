use bevy::{prelude::*, text::EditableText, ui::InteractionDisabled};

pub(super) struct TextField {
    pub(super) label: &'static str,
    pub(super) required: bool,
}

impl TextField {
    pub(super) fn display_label(&self) -> String {
        if self.required {
            format!("{}*", self.label)
        } else {
            self.label.to_owned()
        }
    }

    pub(super) fn read(&self, input: &EditableText) -> Option<String> {
        let value = input.value().to_string();
        let trimmed = value.trim();
        (!self.required || !trimmed.is_empty()).then(|| trimmed.to_owned())
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
        commands.entity(button).insert(InteractionDisabled);
    }
}
