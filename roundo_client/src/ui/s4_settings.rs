use super::{
    UiAction, key_bindings,
    widgets::{
        BUTTON_HOVERED, BUTTON_PRESSED, UiButtonPalette, spawn_compact_button, spawn_heading,
        spawn_label,
    },
};
use crate::targeting::ClientVoxelRaycastSettings;
use bevy::{
    input_focus::{FocusLost, InputFocus},
    picking::hover::Hovered,
    prelude::*,
    text::{EditableText, EditableTextFilter, TextCursorStyle},
    ui_widgets::{
        SelectAllOnFocus, Slider, SliderDragState, SliderPrecision, SliderRange, SliderStep,
        SliderThumb, SliderValue, TrackClick, ValueChange, observe,
    },
};
use roundo_marionette::{ClientKeyBindings, ClientMarionetteInputSettings, MovementAction};
use roundo_user_config::{
    DEFAULT_CAMERA_MOVE_SPEED, DEFAULT_MOUSE_SENSITIVITY, DEFAULT_VOXEL_RAYCAST_DISTANCE,
    MAX_CAMERA_MOVE_SPEED, MAX_MOUSE_SENSITIVITY, MAX_VOXEL_RAYCAST_DISTANCE,
    MIN_CAMERA_MOVE_SPEED, MIN_MOUSE_SENSITIVITY, MIN_VOXEL_RAYCAST_DISTANCE,
};

const SETTINGS_BACKGROUND: Color = Color::srgb(0.025, 0.032, 0.046);
const PANEL_BACKGROUND: Color = Color::srgb(0.06, 0.073, 0.1);
const GROUP_BACKGROUND: Color = Color::srgb(0.085, 0.1, 0.135);
const ACCENT: Color = Color::srgb(0.24, 0.68, 0.54);
const ACCENT_HOVERED: Color = Color::srgb(0.3, 0.76, 0.62);
const ACCENT_PRESSED: Color = Color::srgb(0.18, 0.55, 0.43);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SettingsTab {
    #[default]
    Controls,
    Camera,
    KeyBindings,
}

impl SettingsTab {
    const ALL: [Self; 3] = [Self::Controls, Self::Camera, Self::KeyBindings];

    const fn label(self) -> &'static str {
        match self {
            Self::Controls => "Controls",
            Self::Camera => "Camera",
            Self::KeyBindings => "Key Bindings",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SettingsGroup {
    Mouse,
    CameraMovement,
    Targeting,
}

impl SettingsGroup {
    const fn label(self) -> &'static str {
        match self {
            Self::Mouse => "Mouse",
            Self::CameraMovement => "Movement",
            Self::Targeting => "Targeting",
        }
    }
}

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TunableSetting {
    MouseSensitivity,
    CameraSpeed,
    VoxelRaycastDistance,
}

impl TunableSetting {
    const ALL: [Self; 3] = [
        Self::MouseSensitivity,
        Self::CameraSpeed,
        Self::VoxelRaycastDistance,
    ];

    const fn spec(self) -> TunableSettingSpec {
        match self {
            Self::MouseSensitivity => TunableSettingSpec {
                label: "Mouse sensitivity",
                description: "Look rotation per pointer movement",
                tab: SettingsTab::Controls,
                group: SettingsGroup::Mouse,
                default: DEFAULT_MOUSE_SENSITIVITY,
                minimum: MIN_MOUSE_SENSITIVITY,
                maximum: MAX_MOUSE_SENSITIVITY,
                step: 0.0005,
                decimals: 4,
            },
            Self::CameraSpeed => TunableSettingSpec {
                label: "Camera speed",
                description: "Free camera movement speed",
                tab: SettingsTab::Camera,
                group: SettingsGroup::CameraMovement,
                default: DEFAULT_CAMERA_MOVE_SPEED,
                minimum: MIN_CAMERA_MOVE_SPEED,
                maximum: MAX_CAMERA_MOVE_SPEED,
                step: 1.0,
                decimals: 1,
            },
            Self::VoxelRaycastDistance => TunableSettingSpec {
                label: "Voxel ray distance",
                description: "Maximum distance used to inspect the targeted voxel",
                tab: SettingsTab::Camera,
                group: SettingsGroup::Targeting,
                default: DEFAULT_VOXEL_RAYCAST_DISTANCE,
                minimum: MIN_VOXEL_RAYCAST_DISTANCE,
                maximum: MAX_VOXEL_RAYCAST_DISTANCE,
                step: 1.0,
                decimals: 1,
            },
        }
    }

    pub(super) fn value(
        self,
        settings: &ClientMarionetteInputSettings,
        raycast: &ClientVoxelRaycastSettings,
    ) -> f32 {
        match self {
            Self::MouseSensitivity => settings.mouse_sensitivity,
            Self::CameraSpeed => settings.camera_move_speed,
            Self::VoxelRaycastDistance => raycast.max_distance(),
        }
    }

    pub(super) fn set(
        self,
        settings: &mut ClientMarionetteInputSettings,
        raycast: &mut ClientVoxelRaycastSettings,
        value: f32,
    ) -> f32 {
        let spec = self.spec();
        let value = if value.is_finite() {
            value.clamp(spec.minimum, spec.maximum)
        } else {
            spec.default
        };
        match self {
            Self::MouseSensitivity => settings.mouse_sensitivity = value,
            Self::CameraSpeed => settings.camera_move_speed = value,
            Self::VoxelRaycastDistance => {
                raycast.set_max_distance(value);
            }
        }
        value
    }

    pub(super) fn adjust(
        self,
        settings: &mut ClientMarionetteInputSettings,
        raycast: &mut ClientVoxelRaycastSettings,
        direction: f32,
    ) {
        let value = self.value(settings, raycast) + self.spec().step * direction;
        self.set(settings, raycast, value);
    }

    fn format(
        self,
        settings: &ClientMarionetteInputSettings,
        raycast: &ClientVoxelRaycastSettings,
    ) -> String {
        let spec = self.spec();
        format!("{:.*}", spec.decimals, self.value(settings, raycast))
    }
}

#[derive(Clone, Copy)]
struct TunableSettingSpec {
    label: &'static str,
    description: &'static str,
    tab: SettingsTab,
    group: SettingsGroup,
    default: f32,
    minimum: f32,
    maximum: f32,
    step: f32,
    decimals: usize,
}

#[derive(Resource)]
pub(super) struct SettingsUiState {
    pub(super) selected_tab: SettingsTab,
    mouse_expanded: bool,
    camera_movement_expanded: bool,
    targeting_expanded: bool,
    expanded_movement_actions: [bool; MovementAction::ALL.len()],
    selected_key: Option<KeyCode>,
}

impl SettingsUiState {
    pub(super) fn select_tab(&mut self, tab: SettingsTab) -> bool {
        if self.selected_tab == tab {
            return false;
        }
        self.selected_tab = tab;
        self.selected_key = None;
        true
    }

    pub(super) fn toggle_group(&mut self, group: SettingsGroup) {
        let expanded = match group {
            SettingsGroup::Mouse => &mut self.mouse_expanded,
            SettingsGroup::CameraMovement => &mut self.camera_movement_expanded,
            SettingsGroup::Targeting => &mut self.targeting_expanded,
        };
        *expanded = !*expanded;
    }

    fn group_expanded(&self, group: SettingsGroup) -> bool {
        match group {
            SettingsGroup::Mouse => self.mouse_expanded,
            SettingsGroup::CameraMovement => self.camera_movement_expanded,
            SettingsGroup::Targeting => self.targeting_expanded,
        }
    }

    pub(super) fn toggle_movement_action(&mut self, action: MovementAction) {
        let expanded = &mut self.expanded_movement_actions[action.index()];
        *expanded = !*expanded;
    }

    pub(super) fn movement_action_expanded(&self, action: MovementAction) -> bool {
        self.expanded_movement_actions[action.index()]
    }

    pub(super) fn open_key_queue(&mut self, key: KeyCode) {
        self.selected_key = Some(key);
    }

    pub(super) fn close_key_queue(&mut self) {
        self.selected_key = None;
    }

    pub(super) fn selected_key(&self) -> Option<KeyCode> {
        self.selected_key
    }
}

impl Default for SettingsUiState {
    fn default() -> Self {
        Self {
            selected_tab: SettingsTab::Controls,
            mouse_expanded: true,
            camera_movement_expanded: true,
            targeting_expanded: true,
            expanded_movement_actions: [false; MovementAction::ALL.len()],
            selected_key: None,
        }
    }
}

#[derive(Component)]
pub(super) struct SettingsSliderThumb;

#[derive(Component)]
struct SettingsCanvas;

pub(super) fn spawn(
    commands: &mut Commands,
    canvas: Entity,
    settings: &ClientMarionetteInputSettings,
    raycast: &ClientVoxelRaycastSettings,
    bindings: &ClientKeyBindings,
    ui_state: &SettingsUiState,
) {
    commands
        .entity(canvas)
        .insert((SettingsCanvas, BackgroundColor(SETTINGS_BACKGROUND)));

    let shell = commands
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            padding: UiRect::axes(px(28), px(22)),
            row_gap: px(18),
            ..default()
        })
        .id();
    commands.entity(canvas).add_child(shell);

    spawn_header(commands, shell);
    spawn_tabs(commands, shell, ui_state.selected_tab);

    let content = commands
        .spawn((
            Node {
                width: percent(100),
                max_width: px(860),
                flex_grow: 1.0,
                align_self: AlignSelf::Center,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                row_gap: px(10),
                padding: px(18).all(),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(10)),
                ..default()
            },
            BackgroundColor(PANEL_BACKGROUND),
            BorderColor::all(Color::srgb(0.18, 0.22, 0.3)),
        ))
        .id();
    commands.entity(shell).add_child(content);

    match ui_state.selected_tab {
        SettingsTab::Controls => {
            spawn_group(
                commands,
                content,
                SettingsGroup::Mouse,
                settings,
                raycast,
                ui_state,
            );
        }
        SettingsTab::Camera => {
            spawn_group(
                commands,
                content,
                SettingsGroup::CameraMovement,
                settings,
                raycast,
                ui_state,
            );
            spawn_group(
                commands,
                content,
                SettingsGroup::Targeting,
                settings,
                raycast,
                ui_state,
            );
        }
        SettingsTab::KeyBindings => {
            key_bindings::spawn(commands, content, canvas, bindings, ui_state);
        }
    }
}

fn spawn_header(commands: &mut Commands, parent: Entity) {
    let header = commands
        .spawn(Node {
            width: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        })
        .id();
    commands.entity(parent).add_child(header);
    spawn_heading(commands, header, "Settings");
    spawn_compact_button(commands, header, "Back", UiAction::CloseSettings, px(96));
}

fn spawn_tabs(commands: &mut Commands, parent: Entity, selected: SettingsTab) {
    let tab_row = commands
        .spawn(Node {
            width: percent(100),
            max_width: px(860),
            align_self: AlignSelf::Center,
            column_gap: px(8),
            ..default()
        })
        .id();
    commands.entity(parent).add_child(tab_row);

    for tab in SettingsTab::ALL {
        let button = spawn_compact_button(
            commands,
            tab_row,
            tab.label(),
            UiAction::SelectSettingsTab(tab),
            px(150),
        );
        if tab == selected {
            commands.entity(button).insert((
                BackgroundColor(ACCENT),
                BorderColor::all(ACCENT_HOVERED),
                UiButtonPalette {
                    normal: ACCENT,
                    hovered: ACCENT_HOVERED,
                    pressed: ACCENT_PRESSED,
                },
            ));
        }
    }
}

fn spawn_group(
    commands: &mut Commands,
    parent: Entity,
    group: SettingsGroup,
    settings: &ClientMarionetteInputSettings,
    raycast: &ClientVoxelRaycastSettings,
    ui_state: &SettingsUiState,
) {
    let expanded = ui_state.group_expanded(group);
    let group_panel = commands
        .spawn((
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                border: px(1).all(),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(GROUP_BACKGROUND),
            BorderColor::all(Color::srgb(0.2, 0.24, 0.32)),
        ))
        .id();
    commands.entity(parent).add_child(group_panel);

    let prefix = if expanded { "−" } else { "+" };
    let header = spawn_compact_button(
        commands,
        group_panel,
        &format!("{prefix}  {}", group.label()),
        UiAction::ToggleSettingsGroup(group),
        percent(100),
    );
    commands.entity(header).insert((
        Node {
            width: percent(100),
            height: px(40),
            justify_content: JustifyContent::FlexStart,
            padding: UiRect::horizontal(px(14)),
            border_radius: BorderRadius::all(px(7)),
            ..default()
        },
        UiButtonPalette {
            normal: GROUP_BACKGROUND,
            hovered: BUTTON_HOVERED,
            pressed: BUTTON_PRESSED,
        },
        BackgroundColor(GROUP_BACKGROUND),
    ));

    if !expanded {
        return;
    }

    let list = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            row_gap: px(6),
            padding: UiRect::new(px(12), px(12), px(6), px(12)),
            ..default()
        })
        .id();
    commands.entity(group_panel).add_child(list);

    let tab = match group {
        SettingsGroup::Mouse => SettingsTab::Controls,
        SettingsGroup::CameraMovement | SettingsGroup::Targeting => SettingsTab::Camera,
    };
    for parameter in TunableSetting::ALL {
        let spec = parameter.spec();
        if spec.tab == tab && spec.group == group {
            spawn_tunable_setting(commands, list, parameter, settings, raycast);
        }
    }
}

fn spawn_tunable_setting(
    commands: &mut Commands,
    parent: Entity,
    parameter: TunableSetting,
    settings: &ClientMarionetteInputSettings,
    raycast: &ClientVoxelRaycastSettings,
) {
    let spec = parameter.spec();
    let row = commands
        .spawn((
            Node {
                width: percent(100),
                min_height: px(64),
                display: Display::Grid,
                grid_template_columns: vec![
                    GridTrack::flex(1.25),
                    GridTrack::px(36.0),
                    GridTrack::flex(1.75),
                    GridTrack::px(36.0),
                    GridTrack::px(92.0),
                ],
                column_gap: px(8),
                align_items: AlignItems::Center,
                padding: UiRect::axes(px(10), px(8)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.055, 0.066, 0.09)),
        ))
        .id();
    commands.entity(parent).add_child(row);

    let label_column = commands
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            ..default()
        })
        .id();
    commands.entity(row).add_child(label_column);
    spawn_label(commands, label_column, spec.label);
    let description = commands
        .spawn((
            Text::new(spec.description),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::srgb(0.47, 0.52, 0.62)),
        ))
        .id();
    commands.entity(label_column).add_child(description);

    spawn_compact_button(
        commands,
        row,
        "−",
        UiAction::AdjustSetting(parameter, -1),
        px(36),
    );
    spawn_slider(commands, row, parameter, settings, raycast);
    spawn_compact_button(
        commands,
        row,
        "+",
        UiAction::AdjustSetting(parameter, 1),
        px(36),
    );
    spawn_value_input(commands, row, parameter, settings, raycast);
}

fn spawn_slider(
    commands: &mut Commands,
    parent: Entity,
    parameter: TunableSetting,
    settings: &ClientMarionetteInputSettings,
    raycast: &ClientVoxelRaycastSettings,
) {
    let spec = parameter.spec();
    let slider = commands
        .spawn((
            parameter,
            Slider {
                track_click: TrackClick::Snap,
                ..default()
            },
            SliderValue(parameter.value(settings, raycast)),
            SliderRange::new(spec.minimum, spec.maximum),
            SliderStep(spec.step),
            SliderPrecision(spec.decimals as i32),
            SliderDragState::default(),
            Hovered::default(),
            Node {
                width: percent(100),
                height: px(20),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            observe(apply_slider_value),
        ))
        .id();
    commands.entity(parent).add_child(slider);

    let rail = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                height: px(5),
                border_radius: BorderRadius::all(px(3)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.15, 0.18, 0.24)),
        ))
        .id();
    commands.entity(slider).add_child(rail);

    let thumb_track = commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(14),
            top: px(0),
            bottom: px(0),
            ..default()
        })
        .id();
    commands.entity(slider).add_child(thumb_track);
    let thumb = commands
        .spawn((
            SettingsSliderThumb,
            SliderThumb,
            Node {
                position_type: PositionType::Absolute,
                left: percent(0),
                top: px(3),
                width: px(14),
                height: px(14),
                border_radius: BorderRadius::MAX,
                ..default()
            },
            BackgroundColor(ACCENT),
        ))
        .id();
    commands.entity(thumb_track).add_child(thumb);
}

fn spawn_value_input(
    commands: &mut Commands,
    parent: Entity,
    parameter: TunableSetting,
    settings: &ClientMarionetteInputSettings,
    raycast: &ClientVoxelRaycastSettings,
) {
    let input = commands
        .spawn((
            parameter,
            EditableText::new(parameter.format(settings, raycast)),
            EditableTextFilter::new(|character| character.is_ascii_digit() || character == '.'),
            TextCursorStyle::default(),
            SelectAllOnFocus,
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(15.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                width: px(92),
                height: px(36),
                padding: UiRect::axes(px(8), px(7)),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(5)),
                overflow: Overflow::clip_x(),
                ..default()
            },
            BackgroundColor(Color::srgb(0.025, 0.031, 0.043)),
            BorderColor::all(Color::srgb(0.29, 0.34, 0.43)),
            observe(commit_value_input_on_focus_loss),
        ))
        .id();
    commands.entity(parent).add_child(input);
}

fn apply_slider_value(
    value_change: On<ValueChange<f32>>,
    parameters: Query<&TunableSetting>,
    mut settings: ResMut<ClientMarionetteInputSettings>,
    mut raycast: ResMut<ClientVoxelRaycastSettings>,
    mut commands: Commands,
) {
    let Ok(parameter) = parameters.get(value_change.source) else {
        return;
    };
    let value = parameter.set(&mut settings, &mut raycast, value_change.value);
    commands
        .entity(value_change.source)
        .insert(SliderValue(value));
}

fn commit_value_input_on_focus_loss(
    focus_lost: On<FocusLost>,
    mut inputs: Query<(&TunableSetting, &mut EditableText)>,
    mut settings: ResMut<ClientMarionetteInputSettings>,
    mut raycast: ResMut<ClientVoxelRaycastSettings>,
) {
    let Ok((parameter, mut input)) = inputs.get_mut(focus_lost.event_target()) else {
        return;
    };
    commit_value_input(*parameter, &mut input, &mut settings, &mut raycast);
}

pub(super) fn commit_value_input_on_enter(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut input_focus: ResMut<InputFocus>,
    mut inputs: Query<(&TunableSetting, &mut EditableText)>,
    mut settings: ResMut<ClientMarionetteInputSettings>,
    mut raycast: ResMut<ClientVoxelRaycastSettings>,
) {
    if !keyboard.just_pressed(KeyCode::Enter) {
        return;
    }
    let Some(focused) = input_focus.get() else {
        return;
    };
    let Ok((parameter, mut input)) = inputs.get_mut(focused) else {
        return;
    };
    commit_value_input(*parameter, &mut input, &mut settings, &mut raycast);
    input_focus.clear();
}

fn commit_value_input(
    parameter: TunableSetting,
    input: &mut EditableText,
    settings: &mut ClientMarionetteInputSettings,
    raycast: &mut ClientVoxelRaycastSettings,
) {
    if let Ok(value) = input.value().to_string().parse::<f32>() {
        parameter.set(settings, raycast, value);
    }
    input
        .editor_mut()
        .set_text(&parameter.format(settings, raycast));
}

pub(super) fn sync_setting_controls(
    settings: Res<ClientMarionetteInputSettings>,
    raycast: Res<ClientVoxelRaycastSettings>,
    input_focus: Res<InputFocus>,
    sliders: Query<(Entity, &TunableSetting, &SliderValue)>,
    mut inputs: Query<(Entity, &TunableSetting, &mut EditableText), Without<Slider>>,
    mut commands: Commands,
) {
    if !settings.is_changed() && !raycast.is_changed() {
        return;
    }

    for (entity, parameter, slider_value) in &sliders {
        let value = parameter.value(&settings, &raycast);
        if (slider_value.0 - value).abs() > f32::EPSILON {
            commands.entity(entity).insert(SliderValue(value));
        }
    }

    for (entity, parameter, mut input) in &mut inputs {
        if input_focus.get() == Some(entity) {
            continue;
        }
        let formatted = parameter.format(&settings, &raycast);
        if input.value().to_string() != formatted {
            input.editor_mut().set_text(&formatted);
        }
    }
}

pub(super) fn update_slider_visuals(
    sliders: Query<
        (
            Entity,
            &SliderValue,
            &SliderRange,
            &Hovered,
            &SliderDragState,
        ),
        Or<(
            Changed<SliderValue>,
            Changed<Hovered>,
            Changed<SliderDragState>,
        )>,
    >,
    children: Query<&Children>,
    mut thumbs: Query<(&mut Node, &mut BackgroundColor), With<SettingsSliderThumb>>,
) {
    for (slider, value, range, hovered, drag_state) in &sliders {
        for descendant in children.iter_descendants(slider) {
            let Ok((mut node, mut color)) = thumbs.get_mut(descendant) else {
                continue;
            };
            node.left = percent(range.thumb_position(value.0) * 100.0);
            color.0 = if hovered.0 || drag_state.dragging {
                ACCENT_HOVERED
            } else {
                ACCENT
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TunableSetting;
    use crate::targeting::ClientVoxelRaycastSettings;
    use roundo_marionette::ClientMarionetteInputSettings;
    use roundo_user_config::{DEFAULT_CAMERA_MOVE_SPEED, DEFAULT_MOUSE_SENSITIVITY};

    #[test]
    fn tunable_settings_share_clamping_and_adjustment_behavior() {
        let mut settings = ClientMarionetteInputSettings::default();
        let mut raycast = ClientVoxelRaycastSettings::default();

        TunableSetting::MouseSensitivity.set(&mut settings, &mut raycast, 100.0);
        TunableSetting::CameraSpeed.set(&mut settings, &mut raycast, -100.0);
        TunableSetting::VoxelRaycastDistance.set(&mut settings, &mut raycast, 100.0);
        assert_eq!(settings.mouse_sensitivity, 0.01);
        assert_eq!(settings.camera_move_speed, 1.0);
        assert_eq!(raycast.max_distance(), 16.0);

        TunableSetting::MouseSensitivity.adjust(&mut settings, &mut raycast, -1.0);
        TunableSetting::CameraSpeed.adjust(&mut settings, &mut raycast, 1.0);
        TunableSetting::VoxelRaycastDistance.adjust(&mut settings, &mut raycast, -1.0);
        assert!((settings.mouse_sensitivity - 0.0095).abs() < f32::EPSILON);
        assert_eq!(settings.camera_move_speed, 2.0);
        assert_eq!(raycast.max_distance(), 15.0);

        TunableSetting::MouseSensitivity.set(&mut settings, &mut raycast, f32::NAN);
        TunableSetting::CameraSpeed.set(&mut settings, &mut raycast, f32::INFINITY);
        assert_eq!(settings.mouse_sensitivity, DEFAULT_MOUSE_SENSITIVITY);
        assert_eq!(settings.camera_move_speed, DEFAULT_CAMERA_MOVE_SPEED);
    }
}
