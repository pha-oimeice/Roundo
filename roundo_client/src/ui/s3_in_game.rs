use crate::targeting::ClientVoxelTarget;
use bevy::{
    picking::Pickable,
    prelude::*,
    window::{PrimaryWindow, Window},
};
use roundo_local_coordinate::LocalCoordinateClientWorld;

const CROSSHAIR_SIZE: f32 = 24.0;
const CROSSHAIR_ARM_LENGTH: f32 = 8.0;
const CROSSHAIR_THICKNESS: f32 = 2.0;
const CROSSHAIR_GAP: f32 = 3.0;
const CROSSHAIR_COLOR: Color = Color::srgba(0.9, 0.96, 1.0, 0.92);
const WAILA_BACKGROUND: Color = Color::srgba(0.025, 0.032, 0.046, 0.78);

#[derive(Component)]
pub(super) struct Crosshair;

#[derive(Component)]
pub(super) struct WailaPanel;

#[derive(Component)]
pub(super) struct WailaText;

pub(super) fn spawn(commands: &mut Commands, canvas: Entity) {
    commands.entity(canvas).insert(Pickable::IGNORE);
    spawn_waila(commands, canvas);
    let crosshair = commands
        .spawn((
            Crosshair,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                width: px(CROSSHAIR_SIZE),
                height: px(CROSSHAIR_SIZE),
                ..default()
            },
        ))
        .id();
    commands.entity(canvas).add_child(crosshair);

    for node in crosshair_arms() {
        let arm = commands
            .spawn((Pickable::IGNORE, node, BackgroundColor(CROSSHAIR_COLOR)))
            .id();
        commands.entity(crosshair).add_child(arm);
    }
}

fn spawn_waila(commands: &mut Commands, canvas: Entity) {
    let wrapper = commands
        .spawn((
            WailaPanel,
            Pickable::IGNORE,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: px(24),
                left: px(0),
                right: px(0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .id();
    commands.entity(canvas).add_child(wrapper);

    let panel = commands
        .spawn((
            Pickable::IGNORE,
            Node {
                min_width: px(360),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                padding: UiRect::axes(px(16), px(12)),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(WAILA_BACKGROUND),
            BorderColor::all(Color::srgba(0.78, 0.84, 0.94, 0.32)),
        ))
        .id();
    commands.entity(wrapper).add_child(panel);

    let text = commands
        .spawn((
            WailaText,
            Pickable::IGNORE,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    commands.entity(panel).add_child(text);
}

pub(super) fn sync_waila(
    target: Option<Res<ClientVoxelTarget>>,
    local_coordinates: Option<Res<LocalCoordinateClientWorld>>,
    mut panels: Query<&mut Visibility, With<WailaPanel>>,
    mut texts: Query<&mut Text, With<WailaText>>,
) {
    let Some(hit) = target.as_deref().and_then(|target| target.hit) else {
        for mut visibility in &mut panels {
            *visibility = Visibility::Hidden;
        }
        return;
    };

    let local_coordinate = local_coordinates
        .as_deref()
        .and_then(|world| world.local_coordinate_id(hit.chunk.local_coordinate_entity()))
        .map(|id| id.0.to_string())
        .unwrap_or_else(|| format!("{:?}", hit.chunk.local_coordinate_entity()));
    let voxel = hit.voxel.position;
    let relative = hit.voxel_relative_position;
    let chunk = hit.chunk.local_chunk_position();
    let value = format!(
        "Block  ·  material {}\nVoxel: [{}, {}, {}]  ·  Relative: [{}, {}, {}]\nChunk: [{}, {}, {}]  ·  LocalCoordinate: {}",
        hit.voxel.data.id,
        voxel.x,
        voxel.y,
        voxel.z,
        relative.x,
        relative.y,
        relative.z,
        chunk.x,
        chunk.y,
        chunk.z,
        local_coordinate,
    );
    for mut text in &mut texts {
        if text.0 != value {
            text.0.clone_from(&value);
        }
    }
    for mut visibility in &mut panels {
        *visibility = Visibility::Inherited;
    }
}

pub(super) fn follow_pointer(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut crosshairs: Query<&mut Node, With<Crosshair>>,
) {
    let Some(window) = windows.iter().next() else {
        return;
    };
    let Some(pointer) = window.cursor_position() else {
        return;
    };

    for mut node in &mut crosshairs {
        node.left = px(pointer.x - CROSSHAIR_SIZE * 0.5);
        node.top = px(pointer.y - CROSSHAIR_SIZE * 0.5);
    }
}

fn crosshair_arms() -> [Node; 4] {
    let center = CROSSHAIR_SIZE * 0.5;
    [
        Node {
            position_type: PositionType::Absolute,
            width: px(CROSSHAIR_ARM_LENGTH),
            height: px(CROSSHAIR_THICKNESS),
            left: px(center - CROSSHAIR_GAP - CROSSHAIR_ARM_LENGTH),
            top: px(center - CROSSHAIR_THICKNESS * 0.5),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            width: px(CROSSHAIR_ARM_LENGTH),
            height: px(CROSSHAIR_THICKNESS),
            left: px(center + CROSSHAIR_GAP),
            top: px(center - CROSSHAIR_THICKNESS * 0.5),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            width: px(CROSSHAIR_THICKNESS),
            height: px(CROSSHAIR_ARM_LENGTH),
            left: px(center - CROSSHAIR_THICKNESS * 0.5),
            top: px(center - CROSSHAIR_GAP - CROSSHAIR_ARM_LENGTH),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            width: px(CROSSHAIR_THICKNESS),
            height: px(CROSSHAIR_ARM_LENGTH),
            left: px(center - CROSSHAIR_THICKNESS * 0.5),
            top: px(center + CROSSHAIR_GAP),
            ..default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{CROSSHAIR_SIZE, Crosshair, follow_pointer};
    use bevy::{prelude::*, window::PrimaryWindow};

    #[test]
    fn crosshair_uses_the_current_pointer_position() {
        let mut app = App::new();
        app.add_systems(Update, follow_pointer);

        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(321.0, 187.0)));
        app.world_mut().spawn((window, PrimaryWindow));
        let crosshair = app.world_mut().spawn((Crosshair, Node::default())).id();

        app.update();

        let node = app.world().get::<Node>(crosshair).unwrap();
        assert_eq!(node.left, px(321.0 - CROSSHAIR_SIZE * 0.5));
        assert_eq!(node.top, px(187.0 - CROSSHAIR_SIZE * 0.5));
    }
}
