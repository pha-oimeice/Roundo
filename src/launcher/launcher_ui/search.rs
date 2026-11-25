use bevy::{
    feathers::{
        controls::{FeathersTextInput, FeathersTextInputContainer},
        font_styles::InheritableFont,
    },
    prelude::*,
    scene::Scene,
    text::{EditableText, TextEditChange},
};

use super::menu_emoji_scene;

#[derive(Clone, Component, Default)]
pub(super) struct SearchQuery(String);

impl SearchQuery {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn matches(&self, text: &str) -> bool {
        self.is_empty() || text.to_lowercase().contains(&self.0)
    }
}

pub(super) fn search_field_scene(
    font: Handle<Font>,
    emoji: Handle<Font>,
    label: &'static str,
    marker: impl Scene,
) -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        Node {
            width: percent(100), min_width: px(0), max_width: px(420), height: px(36),
            flex_grow: 0.0, flex_shrink: 0.0, align_self: AlignSelf::FlexStart,
        }
        InheritableFont { font: font, font_size: FontSize::Px(16.0) }
        Children [
            @menu_emoji_scene(emoji, "🔍", 16.0)
            --
            @FeathersTextInput
            AccessibleLabel(label)
            Node { min_width: px(0) }
            SearchQuery
            @{marker}
            on(update_search_query)
        ]
    }
}

fn update_search_query(
    event: On<TextEditChange>,
    mut inputs: Query<(&EditableText, &mut SearchQuery)>,
) {
    let (text, mut query) = inputs.get_mut(event.event_target()).expect("search field");
    let normalized = text.value().to_string().to_lowercase();
    if query.0 != normalized {
        query.0 = normalized;
    }
}
