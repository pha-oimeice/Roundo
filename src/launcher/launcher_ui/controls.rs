use std::collections::BTreeSet;

use bevy::{
    ecs::template::{EntityTemplate, FromTemplate, OptionTemplate},
    feathers::{
        controls::{ButtonVariant, FeathersButton},
        display::caption,
        focus::FocusIndicator,
        font_styles::InheritableFont,
    },
    input::mouse::AccumulatedMouseMotion,
    input_focus::{
        FocusCause, InputFocus,
        tab_navigation::{TabGroup, TabIndex},
    },
    picking::{events::PointerClick, pointer::PointerButton},
    prelude::*,
    scene::Scene,
    text::{FontSourceTemplate, Underline},
    ui::{FixedNode, InteractionDisabled, OverrideClip},
    ui_widgets::{Activate, Button, SelectedTab, Tab, TabActivation, TabList, ValueChange},
};
use bevy_enhanced_input::prelude::Binding;
use roundo_infrastructure::user_config::{ClientInputKey as Key, ClientSettingsConfig};
use roundo_marionette::LOOK_FUNCTION;

use super::super::{
    config_store::ClientConfigStore,
    local_input::{LocalBindings, physical_binding},
};
use super::{
    BACKGROUND, DIVIDER, LauncherFont,
    highlight::{Highlight, set_button_enabled},
    launcher_label_with_font, launcher_scroll_area_scene, primary_pointer_target_scene,
    search::{SearchQuery, search_field_scene},
};

mod device_layout;
mod mouse_diagram;
use device_layout::{KEYBOARD, PhysicalKey, key_label};

const DEVICE_WINDOW_ASPECT: f32 = 45.5 / 14.95;
const BOUND_KEY_BORDER: Color = Color::srgb_u8(89, 113, 143);
const UNBOUND_KEY_BORDER: Color = Color::srgb_u8(44, 56, 71);
const UNBOUND_KEY_TEXT: Color = Color::srgb_u8(134, 148, 166);

pub(super) struct ControlsPlugin;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SystemSet)]
pub(super) struct ControlsCapture;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ControlsState>().add_systems(
            Update,
            (capture_binding.in_set(ControlsCapture), update_controls).chain(),
        );
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Component)]
enum Device {
    #[default]
    Keyboard,
    Mouse,
    Gamepad,
}

#[derive(Resource, Default)]
struct ControlsState {
    device: Device,
    selected_category: Option<usize>,
    action: Option<String>,
    key_filter: Option<Key>,
    collapsed: BTreeSet<usize>,
    filtered_collapsed: BTreeSet<usize>,
    capture: Option<Capture>,
    error: String,
}

struct Capture {
    action: String,
    previous: Option<Key>,
    ready: bool,
}

#[derive(Component, FromTemplate)]
struct ActionRow(String);

#[derive(Component, FromTemplate)]
struct Category(usize);

#[derive(Component, FromTemplate)]
struct CategoryIndicator(usize);

#[derive(Component, FromTemplate)]
struct ActionGroup(usize);

#[derive(Component, FromTemplate)]
struct DevicePanel(Device);

#[derive(Clone, Component, Default)]
struct DeviceKey(Option<Key>);

#[derive(Clone, Component, Default)]
struct DeviceKeyLabel;

#[derive(Clone, Component, Default)]
struct ControlsPage;

#[derive(Clone, Component, Default)]
struct ActionSearch;

#[derive(Clone, Component, Default)]
struct BindingRows;

#[derive(Clone, Component, Default)]
struct BindingRow;

#[derive(Clone, Component, Default)]
struct AddBinding;

#[derive(Clone, Component, Default)]
struct BindingStatus;

#[derive(Clone, Component, Default)]
struct CaptureOverlay;

#[derive(Clone, Component, Default)]
struct CapturePrompt;

pub(super) fn controls_scene(
    settings: &ClientSettingsConfig,
    font: Handle<Font>,
    emoji: Handle<Font>,
) -> impl Scene {
    let categories = settings
        .input_categories
        .iter()
        .enumerate()
        .map(|(index, category)| {
            let actions = category
                .actions
                .iter()
                .map(|id| {
                    action_scene(
                        font.clone(),
                        id.clone(),
                        settings.input_slots[id].display_name.clone(),
                    )
                })
                .collect::<Vec<_>>();
            category_scene(font.clone(), index, category.name.clone(), actions)
        })
        .collect::<Vec<_>>();
    let action_list = launcher_scroll_area_scene(bsn_list! { {categories} });
    let bindings = launcher_scroll_area_scene(bsn_list! {
        BindingRows
        Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, flex_shrink: 0.0, column_gap: px(6), row_gap: px(6) }
        Children [ @add_binding_button_scene(font.clone()) ]
    });
    bsn! {
        ControlsPage
        Node { flex_grow: 1.0, min_height: px(0), min_width: px(0), column_gap: px(12) }
        Children [
            Node {
                width: percent(30), min_width: px(104), max_width: px(260), flex_shrink: 0.0,
                flex_direction: FlexDirection::Column, row_gap: px(8),
                padding: {UiRect::right(px(10))}, border: {UiRect::right(px(1))},
            }
            BorderColor::all(DIVIDER)
            Children [
                @controls_heading_scene(font.clone(), "Inputs")
                --
                @{action_list}
                on(clear_action_on_blank)
                --
                @search_field_scene(font.clone(), emoji, "Search actions by name", bsn! { ActionSearch })
            ]
            --
            Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(8) }
            Children [
                Node {
                    flex_direction: FlexDirection::Column, row_gap: px(6),
                    width: percent(100), max_width: vh(50.0 * DEVICE_WINDOW_ASPECT),
                    aspect_ratio: {Some(DEVICE_WINDOW_ASPECT)}, flex_shrink: 0.0,
                    min_height: px(0), padding: {UiRect::all(px(6))},
                    border: {UiRect::all(px(1))},
                }
                BorderColor::all(DIVIDER)
                Children [
                    @device_tab_list_scene(#KeyboardDevice)
                    Children [
                        #KeyboardDevice
                        @device_tab(font.clone(), Device::Keyboard, "Keyboard")
                        --
                        @device_tab(font.clone(), Device::Mouse, "Mouse")
                        --
                        @device_tab(font.clone(), Device::Gamepad, "Gamepad")
                    ]
                    --
                    @device_panel(Device::Keyboard)
                    Children [ {KEYBOARD.iter().map(|&key| key_scene(font.clone(), key)).collect::<Vec<_>>()} ]
                    --
                    @device_panel(Device::Mouse)
                    Children [ @mouse_diagram::scene() ]
                    --
                    @device_panel(Device::Gamepad)
                ]
                --
                Node {
                    flex_grow: 1.0, flex_basis: px(0), min_height: px(0),
                    flex_direction: FlexDirection::Column, row_gap: px(6),
                    padding: {UiRect::all(px(6))}, border: {UiRect::all(px(1))},
                }
                BorderColor::all(DIVIDER)
                Children [
                    @controls_heading_scene(font.clone(), "Bindings")
                    --
                    @{bindings}
                ]
                --
                @launcher_label_with_font(font.clone(), "", 13.0)
                BindingStatus
            ]
            --
            @capture_overlay_scene(font)
        ]
    }
}

fn controls_heading_scene(font: Handle<Font>, title: &'static str) -> impl Scene {
    bsn! {
        @control_label(font, title, 16.0)
        Underline
        Node { align_self: AlignSelf::FlexStart, flex_shrink: 0.0 }
    }
}

fn control_button(name: impl Into<String>) -> impl Scene {
    control_button_with_hit_insets(name, UiRect::ZERO)
}

fn control_button_with_hit_insets(name: impl Into<String>, insets: UiRect) -> impl Scene {
    bsn! {
        Button
        TabIndex(0)
        FocusIndicator
        Highlight
        AccessibleLabel({name.into()})
        Pickable::IGNORE
        Node { align_items: AlignItems::Center, justify_content: JustifyContent::Center, flex_shrink: 0.0 }
        Children [
            @primary_pointer_target_scene()
            Node { left: {insets.left}, right: {insets.right}, top: {insets.top}, bottom: {insets.bottom} }
        ]
    }
}

fn control_label(font: Handle<Font>, name: impl Into<String>, size: f32) -> impl Scene {
    bsn! {
        @launcher_label_with_font(font, name, size)
        TextLayout::no_wrap()
        Pickable::IGNORE
    }
}

fn category_scene(
    font: Handle<Font>,
    index: usize,
    name: String,
    actions: Vec<impl Scene>,
) -> impl Scene {
    bsn! {
        Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0 }
        Children [
            @control_button(name.clone())
            Category(index)
            Node {
                height: px(34), padding: {UiRect::horizontal(px(4))}, justify_content: JustifyContent::SpaceBetween,
                border: {UiRect::bottom(px(1))},
            }
            BorderColor::all(DIVIDER)
            on(toggle_category)
            Children [
                @control_label(font.clone(), name, 16.0)
                --
                @control_label(font, "−", 18.0)
                CategoryIndicator(index)
            ]
            --
            ActionGroup(index)
            Node { flex_direction: FlexDirection::Column }
            Children [ {actions} ]
        ]
    }
}

fn action_scene(font: Handle<Font>, id: String, name: String) -> impl Scene {
    bsn! {
        @control_button(name.clone())
        ActionRow(id)
        Highlight { idle: Color::NONE }
        Node {
            height: px(30), padding: {UiRect::horizontal(px(6))}, justify_content: JustifyContent::FlexStart,
            border_radius: {BorderRadius::all(px(12))}, overflow: {Overflow::clip()},
        }
        on(toggle_action)
        Children [ @control_label(font, name, 15.0) ]
    }
}

fn device_tab_list_scene(initial: EntityTemplate) -> impl Scene {
    bsn! {
        Node { column_gap: px(4), flex_shrink: 0.0 }
        TabList { activation: TabActivation::Automatic }
        SelectedTab({OptionTemplate::Some(initial)})
        on(switch_device)
    }
}

fn device_tab(font: Handle<Font>, device: Device, title: &'static str) -> impl Scene {
    bsn! {
        Tab
        device
        FocusIndicator
        Highlight
        Node {
            height: px(27), padding: {UiRect::horizontal(px(6))},
            align_items: AlignItems::Center, justify_content: JustifyContent::Center,
        }
        Children [ @control_label(font, title, 12.0) ]
    }
}

fn device_panel(device: Device) -> impl Scene {
    let display = if device == Device::Keyboard {
        Display::Flex
    } else {
        Display::None
    };
    bsn! {
        DevicePanel(device)
        Node { display, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), min_height: px(0), overflow: {Overflow::clip()} }
    }
}

fn key_scene(font: Handle<Font>, key: PhysicalKey) -> impl Scene {
    let [x, y, width, height] = key.rect;
    let node = Node {
        position_type: PositionType::Absolute,
        left: percent(x),
        top: percent(y),
        width: percent(width),
        height: percent(height),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        border: UiRect::all(px(1)),
        border_radius: BorderRadius::all(px(3)),
        overflow: Overflow::clip(),
        ..Default::default()
    };
    let behavior: Box<dyn Scene> = if let Some(input) = key.input {
        Box::new(bsn! {
            @control_button_with_hit_insets(key_label(input), UiRect::all(px(-1)))
            on(toggle_key_filter)
        })
    } else {
        Box::new(bsn! { Highlight })
    };
    bsn! {
        @{behavior}
        DeviceKey({key.input})
        node
        BorderColor::all(DIVIDER)
        Children [
            Text({key.label.to_owned()})
            TextFont { font: {FontSourceTemplate::Handle(font.into())}, font_size: FontSize::Px(10.0) }
            TextLayout::no_wrap()
            Pickable::IGNORE
            DeviceKeyLabel
        ]
    }
}

fn capture_overlay_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        CaptureOverlay
        FixedNode
        OverrideClip
        GlobalZIndex(90)
        TabGroup { modal: true }
        Visibility::Hidden
        Node {
            display: Display::None, position_type: PositionType::Absolute,
            left: px(0), top: px(0), right: px(0), bottom: px(0),
            align_items: AlignItems::Center, justify_content: JustifyContent::Center,
        }
        BackgroundColor({Color::srgba(0.0, 0.0, 0.0, 0.66)})
        Children [
            Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: {UiRect::all(px(18))} }
            BackgroundColor(BACKGROUND)
            Children [
                @launcher_label_with_font(font.clone(), "", 17.0)
                CapturePrompt
                --
                @FeathersButton { @caption: bsn! { @caption("Cancel") }, @variant: ButtonVariant::Normal }
                InheritableFont { font: {font.clone()}, font_size: FontSize::Px(15.0) }
                on(cancel_capture)
            ]
        ]
    }
}

fn toggle_category(
    event: On<Activate>,
    categories: Query<&Category>,
    search: Single<&SearchQuery, With<ActionSearch>>,
    mut state: ResMut<ControlsState>,
) {
    let index = categories.get(event.entity).expect("category header").0;
    state.selected_category = Some(index);
    let collapsed = if is_action_list_filtered(&state, &search) {
        &mut state.filtered_collapsed
    } else {
        &mut state.collapsed
    };
    if !collapsed.remove(&index) {
        collapsed.insert(index);
    }
}

fn toggle_action(event: On<Activate>, rows: Query<&ActionRow>, mut state: ResMut<ControlsState>) {
    let id = &rows.get(event.entity).expect("action row").0;
    if state.action.as_ref() == Some(id) {
        clear_action(&mut state);
    } else {
        state.action = Some(id.clone());
        state.error.clear();
    }
}

fn clear_action_on_blank(event: On<PointerClick>, mut state: ResMut<ControlsState>) {
    if event.button == PointerButton::Primary && state.action.is_some() {
        clear_action(&mut state);
    }
}

fn clear_action(state: &mut ControlsState) {
    state.action = None;
    state.error.clear();
}

fn switch_device(
    event: On<ValueChange<Option<Entity>>>,
    devices: Query<&Device, With<Tab>>,
    mut lists: Query<(&Children, &mut SelectedTab), With<TabList>>,
    mut state: ResMut<ControlsState>,
) {
    let (children, mut selection) = lists.get_mut(event.source).expect("device tab list");
    let tab = event
        .value
        .filter(|tab| children.contains(tab))
        .expect("direct child device tab");
    let device = *devices.get(tab).expect("device tab");
    if device != state.device {
        selection.0 = Some(tab);
        state.device = device;
        state.key_filter = None;
        state.filtered_collapsed.clear();
    }
}

fn is_action_list_filtered(state: &ControlsState, search: &SearchQuery) -> bool {
    !search.is_empty() || state.key_filter.is_some()
}

fn toggle_key_filter(
    event: On<Activate>,
    keys: Query<(&DeviceKey, Has<InteractionDisabled>)>,
    mut state: ResMut<ControlsState>,
) {
    let (key, disabled) = keys.get(event.entity).expect("device key");
    if !disabled {
        let key = key.0.expect("interactive physical key");
        state.key_filter = (state.key_filter != Some(key)).then_some(key);
        state.filtered_collapsed.clear();
    }
}

fn begin_add(_: On<Activate>, mut state: ResMut<ControlsState>) {
    start_capture(&mut state, None);
}

fn start_capture(state: &mut ControlsState, previous: Option<Key>) {
    if state.capture.is_none()
        && let Some(action) = state.action.clone()
    {
        state.capture = Some(Capture {
            action,
            previous,
            ready: false,
        });
        state.error.clear();
    }
}

fn cancel_capture(_: On<Activate>, mut state: ResMut<ControlsState>) {
    state.capture = None;
}

fn capture_binding(
    mut state: ResMut<ControlsState>,
    mut keyboard: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut config: ResMut<ClientConfigStore>,
    mut runtime: ResMut<LocalBindings>,
) {
    let Some(capture) = state.capture.as_ref() else {
        return;
    };
    // Back shortcuts belong to the capture overlay, even while it is arming.
    mouse.clear_just_pressed(MouseButton::Right);
    let escape = keyboard.clear_just_pressed(KeyCode::Escape);
    if !capture.ready {
        state.capture.as_mut().expect("capture started").ready = true;
        return;
    }
    let replacement = if escape {
        None
    } else {
        let key = KEYBOARD
            .iter()
            .filter_map(|key| key.input)
            .chain(mouse_diagram::physical_keys())
            .chain([Key::MouseMotion])
            .find(|key| match physical_binding(*key) {
                Binding::Keyboard { key, .. } => keyboard.just_pressed(key),
                Binding::MouseButton { button, .. } => mouse.just_released(button),
                Binding::MouseMotion { .. } => {
                    config.settings().input_slots[&capture.action]
                        .functions
                        .contains(LOOK_FUNCTION)
                        && motion.delta != Vec2::ZERO
                }
                _ => false,
            });
        let Some(key) = key else { return };
        Some(key)
    };
    let capture = state.capture.take().expect("active capture");
    match config.edit_binding(&capture.action, capture.previous, replacement) {
        Ok(Some(bindings)) => *runtime = bindings,
        Ok(None) => {}
        Err(error) => state.error = error,
    }
}

fn show_node(node: &mut Node, visibility: &mut Visibility, show: bool) {
    let (display, value) = if show {
        (Display::Flex, Visibility::Inherited)
    } else {
        (Display::None, Visibility::Hidden)
    };
    if node.display != display {
        node.display = display;
    }
    if *visibility != value {
        *visibility = value;
    }
}

fn update_controls(
    mut state: ResMut<ControlsState>,
    config: Res<ClientConfigStore>,
    page: (
        Single<Ref<InheritedVisibility>, With<ControlsPage>>,
        Single<Ref<SearchQuery>, With<ActionSearch>>,
    ),
    font: Res<LauncherFont>,
    mut elements: Query<(Entity, &mut Node, &mut Visibility)>,
    actions: Query<(Entity, &ActionRow)>,
    layout: (
        Query<(Entity, &Category, &ChildOf)>,
        Query<(Entity, &ActionGroup)>,
        Query<(Entity, &DevicePanel)>,
    ),
    mut indicators: Query<(&CategoryIndicator, &mut Text)>,
    mut highlights: Query<&mut Highlight>,
    mut keys: Query<(
        Entity,
        &DeviceKey,
        Has<InteractionDisabled>,
        &mut BorderColor,
    )>,
    mut key_labels: Query<(&ChildOf, &mut TextColor), With<DeviceKeyLabel>>,
    mut debug_texts: ParamSet<(
        Single<&mut Text, (With<BindingStatus>, Without<CategoryIndicator>)>,
        Single<&mut Text, (With<CapturePrompt>, Without<CategoryIndicator>)>,
    )>,
    rows: (
        Single<Entity, With<BindingRows>>,
        Query<Entity, With<BindingRow>>,
    ),
    buttons: (
        Single<Entity, With<AddBinding>>,
        Single<Entity, With<CaptureOverlay>>,
    ),
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    let (page, search) = page;
    if search.is_changed() && !state.filtered_collapsed.is_empty() {
        state.filtered_collapsed.clear();
    }
    if !page.get() && state.capture.is_some() {
        state.capture = None;
    }
    if !(state.is_changed() || config.is_changed() || page.is_changed() || search.is_changed()) {
        return;
    }
    let settings = config.settings();
    let bindings = &settings.input_bindings;
    let (add, overlay) = (*buttons.0, *buttons.1);
    if state
        .key_filter
        .is_some_and(|key| !bindings.iter().any(|edge| edge.key == key))
    {
        state.key_filter = None;
        state.filtered_collapsed.clear();
    }
    let selected = state.action.as_deref();
    let bound_keys = bindings
        .iter()
        .filter(|edge| selected.is_none_or(|id| edge.slot == id))
        .map(|edge| edge.key)
        .collect::<BTreeSet<_>>();
    let filtered = is_action_list_filtered(&state, &search);
    let collapsed = if filtered {
        &state.filtered_collapsed
    } else {
        &state.collapsed
    };
    let matches = |id: &str| {
        search.matches(&settings.input_slots[id].display_name)
            && state.key_filter.is_none_or(|key| {
                bindings
                    .iter()
                    .any(|edge| edge.slot == id && edge.key == key)
            })
    };
    for (entity, action) in actions {
        let (_, mut node, mut visibility) = elements.get_mut(entity).expect("action node");
        show_node(&mut node, &mut visibility, matches(&action.0));
        let mut highlight = highlights.get_mut(entity).expect("action highlight");
        highlight.selected = selected == Some(action.0.as_str());
    }
    for (header, category, parent) in &layout.0 {
        let (_, mut node, mut visibility) =
            elements.get_mut(parent.parent()).expect("category node");
        let mut highlight = highlights.get_mut(header).expect("category highlight");
        highlight.selected = state.selected_category == Some(category.0);
        let visible = !filtered
            || settings.input_categories[category.0]
                .actions
                .iter()
                .any(|id| matches(id));
        show_node(&mut node, &mut visibility, visible);
    }
    for (entity, group) in &layout.1 {
        let (_, mut node, mut visibility) = elements.get_mut(entity).expect("action group node");
        show_node(&mut node, &mut visibility, !collapsed.contains(&group.0));
    }
    for (category, mut indicator) in &mut indicators {
        let symbol = if collapsed.contains(&category.0) {
            "+"
        } else {
            "−"
        };
        if indicator.0 != symbol {
            indicator.0 = symbol.into();
        }
    }
    for (entity, panel) in &layout.2 {
        let (_, mut node, mut visibility) = elements.get_mut(entity).expect("device panel node");
        show_node(&mut node, &mut visibility, panel.0 == state.device);
    }
    let is_bound = |key: &DeviceKey| key.0.is_some_and(|key| bound_keys.contains(&key));
    for (entity, key, disabled, mut border) in &mut keys {
        let bound = is_bound(key);
        set_button_enabled(&mut commands, entity, disabled, bound);
        let selected_key = bound && key.0 == state.key_filter;
        highlights.get_mut(entity).expect("key highlight").selected = selected_key;
        border.set_all(if selected_key {
            Color::WHITE
        } else if bound {
            BOUND_KEY_BORDER
        } else {
            UNBOUND_KEY_BORDER
        });
    }
    for (parent, mut color) in &mut key_labels {
        let (_, key, _, _) = keys
            .get(parent.parent())
            .expect("label belongs to a device key");
        color.0 = if is_bound(key) {
            Color::WHITE
        } else {
            UNBOUND_KEY_TEXT
        };
    }
    let (_, mut node, mut visibility) = elements.get_mut(add).expect("add binding node");
    show_node(&mut node, &mut visibility, selected.is_some());
    let (list, existing) = rows;
    for row in &existing {
        commands.entity(row).despawn();
    }
    if let Some(id) = selected {
        let rows: Vec<_> = bindings
            .iter()
            .filter(|edge| edge.slot == id)
            .map(|edge| {
                commands
                    .spawn_scene(binding_row_scene(font.0.clone(), edge.key))
                    .id()
            })
            .collect();
        commands.entity(*list).insert_children(0, &rows);
    }
    debug_texts.p0().0.clone_from(&state.error);
    let (_, mut node, mut visibility) = elements.get_mut(overlay).expect("capture overlay node");
    let was_open = node.display != Display::None;
    show_node(&mut node, &mut visibility, state.capture.is_some());
    if let Some(capture) = &state.capture {
        let prompt = if capture.previous.is_some() {
            "Press a key or mouse button to replace the binding. Esc removes it."
        } else {
            "Press a key or mouse button to add a binding. Esc cancels."
        };
        debug_texts.p1().0 = if settings.input_slots[&capture.action]
            .functions
            .contains(LOOK_FUNCTION)
        {
            format!("Move the mouse to bind motion. {prompt}")
        } else {
            prompt.into()
        };
        if !was_open {
            focus.set(overlay, FocusCause::Auto);
        }
    } else if was_open && page.get() {
        focus.set(add, FocusCause::Auto);
    }
}

fn add_binding_button_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        @control_button("Add an input binding")
        AddBinding
        Node { display: Display::None, width: px(38), height: px(34), border: {UiRect::all(px(1))} }
        Visibility::Hidden
        BorderColor::all(DIVIDER)
        on(begin_add)
        Children [ @control_label(font, "+", 22.0) ]
    }
}

fn binding_row_scene(font: Handle<Font>, key: Key) -> impl Scene {
    let label = key_label(key);
    bsn! {
        @control_button(label)
        BindingRow
        Node { height: px(34), padding: {UiRect::horizontal(px(10))}, border: {UiRect::all(px(1))} }
        BorderColor::all(DIVIDER)
        on(move |_: On<Activate>, mut state: ResMut<ControlsState>| {
            start_capture(&mut state, Some(key));
        })
        Children [ @control_label(font, label, 15.0) ]
    }
}
