use super::{DIVIDER, launcher_label_with_font, launcher_scroll_area_scene};
use bevy::{
    ecs::template::FromTemplate,
    feathers::{controls::FeathersTextInputContainer, font_styles::InheritableFont},
    prelude::*,
    scene::Scene,
    ui_widgets::Activate,
};

pub(super) fn directory_scene(
    font: Handle<Font>,
    rows: impl Scene,
    add_button: impl Scene,
    form_slot: impl Scene,
    filter_input: impl Scene,
) -> impl Scene {
    let scroll = launcher_scroll_area_scene(bsn_list! {
        @{rows}
        Node { flex_direction: FlexDirection::Column, row_gap: px(8), flex_shrink: 0.0 }
        --
        Node { align_items: AlignItems::Center, column_gap: px(12), flex_shrink: 0.0 }
        Children [
            Node { flex_grow: 1.0, height: px(1) }
            BackgroundColor(DIVIDER)
            --
            @{add_button}
            Node { width: px(88), height: px(36) }
            InheritableFont { font: {font.clone()}, font_size: FontSize::Px(24.0) }
        ]
        --
        @{form_slot}
        Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0 }
    });
    bsn! {
        Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, min_height: px(0), row_gap: px(12) }
        Children [
            @{scroll}
            Node { padding: {UiRect::right(px(12))} }
            --
            @FeathersTextInputContainer
            Node { width: percent(100), max_width: px(420), min_height: px(36), flex_grow: 0.0, flex_shrink: 0.0, align_self: AlignSelf::FlexStart }
            InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
            Children [
                @launcher_label_with_font(font.clone(), "Filter", 16.0)
                --
                @{filter_input}
                Node { min_width: px(0) }
            ]
        ]
    }
}

#[derive(Component, FromTemplate)]
pub(super) struct DirectoryDisclosure {
    pub row: Entity,
    pub body: Entity,
    pub indicator: Entity,
}

pub(super) fn toggle_directory_details(
    event: On<Activate>,
    disclosures: Query<(Entity, &DirectoryDisclosure)>,
    parents: Query<&ChildOf>,
    mut bodies: Query<(&mut Node, &mut Visibility)>,
    mut indicators: Query<&mut Text>,
) {
    let (_, selected) = disclosures.get(event.entity).expect("directory disclosure");
    let list = parents.get(selected.row).expect("directory row parent").0;
    for (button, disclosure) in &disclosures {
        if parents.get(disclosure.row).expect("directory row parent").0 != list {
            continue;
        }
        let (mut body, mut visibility) =
            bodies.get_mut(disclosure.body).expect("directory details");
        let is_open = body.display != Display::None;
        let should_open = button == event.entity && !is_open;
        if should_open == is_open {
            continue;
        }
        let (display, details_visibility, symbol) = if should_open {
            (Display::Flex, Visibility::Inherited, "−")
        } else {
            (Display::None, Visibility::Hidden, "+")
        };
        body.display = display;
        *visibility = details_visibility;
        indicators
            .get_mut(disclosure.indicator)
            .expect("directory indicator")
            .0 = symbol.into();
    }
}
