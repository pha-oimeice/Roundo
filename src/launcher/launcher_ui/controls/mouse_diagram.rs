use bevy::{prelude::*, scene::Scene};
use roundo_infrastructure::user_config::ClientInputKey as Key;

use super::super::{DIVIDER, SIDEBAR};
use super::{DeviceKey, control_button_with_hit_insets, key_label, toggle_key_filter};

const WIDTH: f32 = 256.;
const HEIGHT: f32 = 354.;
const STROKE: f32 = 1.;
const SIDE_WIDTH: f32 = 45.;
const BODY_WIDTH: f32 = WIDTH - SIDE_WIDTH;
const MAIN_HEIGHT: f32 = 168.;
const HALF_BODY_WIDTH: f32 = BODY_WIDTH * 0.5;
const MIDDLE_WIDTH: f32 = 47.;
const CENTER: f32 = SIDE_WIDTH + HALF_BODY_WIDTH;

// Diagram-local x, y, width and height. Shared edges use the same coordinates.
const KEYS: &[(Key, [f32; 4])] = &[
    (
        Key::MouseLeft,
        [SIDE_WIDTH, 0., HALF_BODY_WIDTH, MAIN_HEIGHT],
    ),
    (
        Key::MouseMiddle,
        [CENTER - MIDDLE_WIDTH * 0.5, 41., MIDDLE_WIDTH, 76.],
    ),
    (Key::MouseRight, [CENTER, 0., HALF_BODY_WIDTH, MAIN_HEIGHT]),
    (Key::MouseForward, [0., 90., SIDE_WIDTH, 88.]),
    (Key::MouseBack, [0., 215., SIDE_WIDTH, 78.]),
];

pub(super) fn physical_keys() -> impl Iterator<Item = Key> {
    KEYS.iter().map(|&(key, _)| key)
}

pub(super) fn scene() -> impl Scene {
    let keys: Vec<_> = KEYS
        .iter()
        .map(|&(key, rect)| key_scene(key, rect))
        .collect();
    bsn! {
        Node {
            position_type: PositionType::Absolute, left: percent(36), width: percent(64), height: percent(100),
            align_items: AlignItems::Center, justify_content: JustifyContent::Center,
        }
        Pickable::IGNORE
        Children [
            Node { height: percent(80), aspect_ratio: {Some(WIDTH / HEIGHT)}, flex_shrink: 0.0 }
            Pickable::IGNORE
            Children [
                @body_scene()
                --
                {keys}
            ]
        ]
    }
}

fn body_scene() -> impl Scene {
    let node = rect_node([SIDE_WIDTH, MAIN_HEIGHT, BODY_WIDTH, HEIGHT - MAIN_HEIGHT]);
    bsn! {
        node
        BorderColor::all(DIVIDER)
        BackgroundColor(SIDEBAR)
        Pickable::IGNORE
    }
}

fn key_scene(key: Key, rect: [f32; 4]) -> impl Scene {
    let layer = i32::from(key == Key::MouseMiddle);
    let node = rect_node(rect);
    bsn! {
        @control_button_with_hit_insets(key_label(key), UiRect::all(px(-STROKE)))
        DeviceKey({Some(key)})
        ZIndex(layer)
        node
        BorderColor::all(DIVIDER)
        on(toggle_key_filter)
    }
}

fn rect_node([x, y, width, height]: [f32; 4]) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: percent(x / WIDTH * 100.),
        top: percent(y / HEIGHT * 100.),
        width: percent(width / WIDTH * 100.),
        height: percent(height / HEIGHT * 100.),
        border: UiRect::all(px(STROKE)),
        ..Default::default()
    }
}
