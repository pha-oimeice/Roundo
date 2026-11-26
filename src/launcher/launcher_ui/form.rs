use bevy::text::EditableText;

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
