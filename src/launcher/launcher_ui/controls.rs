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
    ui::{FixedNode, OverrideClip},
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
    ACTIVE_TAB_BACKGROUND, BACKGROUND, DIVIDER, LauncherFont, SIDEBAR, launcher_label_with_font,
    launcher_scroll_area_scene, primary_pointer_target_scene,
    search::{SearchQuery, search_field_scene},
};

mod device_layout;
use device_layout::{KEYBOARD, MOUSE, PhysicalKey, key_label};

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
    action: Option<String>,
    key: Option<Key>,
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
struct ControlsPage;

#[derive(Clone, Component, Default)]
struct ActionSearch;

#[derive(Clone, Component, Default)]
struct BindingRows;

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
                    height: percent(58), min_height: px(0), padding: {UiRect::all(px(6))},
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
                    @device_panel(font.clone(), Device::Keyboard, KEYBOARD)
                    --
                    @device_panel(font.clone(), Device::Mouse, MOUSE)
                    --
                    @device_panel(font.clone(), Device::Gamepad, &[])
                ]
                --
                Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), column_gap: px(8), align_items: AlignItems::Stretch }
                Children [
                    @{bindings}
                    --
                    @control_button("+")
                    AccessibleLabel("Add an input binding")
                    AddBinding
                    Node { display: Display::None, width: px(38), height: px(34), align_self: AlignSelf::FlexStart }
                    Visibility::Hidden
                    BackgroundColor(SIDEBAR)
                    on(begin_add)
                    Children [ @control_label(font.clone(), "+", 22.0) ]
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

fn control_button(name: impl Into<String>) -> impl Scene {
    bsn! {
        Button
        TabIndex(0)
        FocusIndicator
        AccessibleLabel({name.into()})
        Pickable::IGNORE
        Node { align_items: AlignItems::Center, justify_content: JustifyContent::Center, flex_shrink: 0.0 }
        Children [ @primary_pointer_target_scene() ]
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
        Category(index)
        Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0 }
        Children [
            @control_button(name.clone())
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
        Node {
            height: px(30), padding: {UiRect::horizontal(px(6))}, justify_content: JustifyContent::FlexStart,
            border: {UiRect::all(px(1))}, border_radius: {BorderRadius::all(px(12))},
            overflow: {Overflow::clip()},
        }
        BorderColor::all(Color::NONE)
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
        Node {
            height: px(27), padding: {UiRect::horizontal(px(6))},
            align_items: AlignItems::Center, justify_content: JustifyContent::Center,
        }
        BackgroundColor(SIDEBAR)
        Children [ @control_label(font, title, 12.0) ]
    }
}

fn device_panel(font: Handle<Font>, device: Device, keys: &[PhysicalKey]) -> impl Scene {
    let keys = keys
        .iter()
        .map(|key| key_scene(font.clone(), *key))
        .collect::<Vec<_>>();
    let display = if device == Device::Keyboard {
        Display::Flex
    } else {
        Display::None
    };
    bsn! {
        DevicePanel(device)
        Node { display, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), min_height: px(0), overflow: {Overflow::clip()} }
        Children [ {keys} ]
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
    let interactive = key.input.map(|input| {
        bsn! {
            @control_button(key_label(input))
            DeviceKey({Some(input)})
            on(toggle_key_filter)
        }
    });
    bsn! {
        @{interactive}
        node
        BackgroundColor(SIDEBAR)
        BorderColor::all(DIVIDER)
        Children [ @control_label(font, key.label, 10.0) ]
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
    parents: Query<&ChildOf>,
    search: Single<&SearchQuery, With<ActionSearch>>,
    mut state: ResMut<ControlsState>,
) {
    let parent = parents
        .get(event.entity)
        .expect("category header parent")
        .parent();
    let index = categories.get(parent).expect("category header group").0;
    let collapsed = if is_action_list_filtered(&state, &search) {
        &mut state.filtered_collapsed
    } else {
        &mut state.collapsed
    };
    if !collapsed.remove(&index) {
        collapsed.insert(index);
    }
}

fn toggle_action(
    event: On<Activate>,
    rows: Query<&ActionRow>,
    config: Res<ClientConfigStore>,
    mut state: ResMut<ControlsState>,
) {
    let id = &rows.get(event.entity).expect("action row").0;
    if state.action.as_ref() == Some(id) {
        clear_action(&mut state, config.settings());
    } else {
        state.action = Some(id.clone());
        state.filtered_collapsed.clear();
        state.error.clear();
    }
}

fn clear_action_on_blank(
    event: On<PointerClick>,
    config: Res<ClientConfigStore>,
    mut state: ResMut<ControlsState>,
) {
    if event.button == PointerButton::Primary && state.action.is_some() {
        clear_action(&mut state, config.settings());
    }
}

fn clear_action(state: &mut ControlsState, settings: &ClientSettingsConfig) {
    state.action = None;
    state.filtered_collapsed.clear();
    if state
        .key
        .is_some_and(|key| !settings.input_bindings.iter().any(|edge| edge.key == key))
    {
        state.key = None;
    }
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
        state.key = None;
        state.filtered_collapsed.clear();
    }
}

fn is_action_list_filtered(state: &ControlsState, search: &SearchQuery) -> bool {
    !search.is_empty() || (state.action.is_none() && state.key.is_some())
}

fn toggle_key_filter(
    event: On<Activate>,
    keys: Query<&DeviceKey>,
    config: Res<ClientConfigStore>,
    mut state: ResMut<ControlsState>,
) {
    let key = keys
        .get(event.entity)
        .expect("device key")
        .0
        .expect("interactive physical key");
    if state.key == Some(key) {
        state.key = None;
        state.filtered_collapsed.clear();
    } else if state.action.is_some()
        || config
            .settings()
            .input_bindings
            .iter()
            .any(|edge| edge.key == key)
    {
        state.key = Some(key);
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
    if !capture.ready {
        state.capture.as_mut().expect("capture started").ready = true;
        return;
    }
    // A captured right click must not also close the Option window.
    mouse.clear_just_pressed(MouseButton::Right);
    let replacement = if keyboard.clear_just_pressed(KeyCode::Escape) {
        None
    } else {
        let key = KEYBOARD
            .iter()
            .chain(MOUSE)
            .filter_map(|key| key.input)
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
    actions: Query<(Entity, &ActionRow, &mut BorderColor)>,
    layout: (
        Query<(Entity, &Category)>,
        Query<(Entity, &ActionGroup)>,
        Query<(Entity, &DevicePanel)>,
    ),
    mut indicators: Query<(&CategoryIndicator, &mut Text)>,
    mut tabs: Query<(&Device, &mut BackgroundColor), Without<DeviceKey>>,
    mut keys: Query<
        (&DeviceKey, &mut BackgroundColor, &mut BorderColor),
        (Without<Device>, Without<ActionRow>),
    >,
    mut debug_texts: ParamSet<(
        Single<&mut Text, (With<BindingStatus>, Without<CategoryIndicator>)>,
        Single<&mut Text, (With<CapturePrompt>, Without<CategoryIndicator>)>,
    )>,
    rows: Single<Entity, With<BindingRows>>,
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
    let (add, overlay) = (*buttons.0, *buttons.1);
    let selected = state.action.as_deref();
    let filtered = is_action_list_filtered(&state, &search);
    let collapsed = if filtered {
        &state.filtered_collapsed
    } else {
        &state.collapsed
    };
    let matches = |id: &str| {
        search.matches(&settings.input_slots[id].display_name)
            && (selected.is_some()
                || state.key.is_none_or(|key| {
                    settings
                        .input_bindings
                        .iter()
                        .any(|edge| edge.slot == id && edge.key == key)
                }))
    };
    for (entity, action, mut border) in actions {
        let (_, mut node, mut visibility) = elements.get_mut(entity).expect("action node");
        show_node(&mut node, &mut visibility, matches(&action.0));
        *border = BorderColor::all(if selected == Some(action.0.as_str()) {
            Color::WHITE
        } else {
            Color::NONE
        });
    }
    for (entity, category) in &layout.0 {
        let (_, mut node, mut visibility) = elements.get_mut(entity).expect("category node");
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
    for (device, mut background) in &mut tabs {
        background.0 = if *device == state.device {
            ACTIVE_TAB_BACKGROUND
        } else {
            SIDEBAR
        };
    }
    let highlighted: BTreeSet<_> = settings
        .input_bindings
        .iter()
        .filter(|edge| selected.is_none_or(|id| id == edge.slot))
        .map(|edge| edge.key)
        .collect();
    for (key, mut background, mut border) in &mut keys {
        background.0 = if key.0.is_some_and(|key| highlighted.contains(&key)) {
            ACTIVE_TAB_BACKGROUND
        } else {
            SIDEBAR
        };
        *border = BorderColor::all(if key.0.is_some() && state.key == key.0 {
            Color::WHITE
        } else {
            DIVIDER
        });
    }
    let (_, mut node, mut visibility) = elements.get_mut(add).expect("add binding node");
    show_node(&mut node, &mut visibility, selected.is_some());
    commands.entity(*rows).despawn_related::<Children>();
    if let Some(id) = selected {
        for edge in settings
            .input_bindings
            .iter()
            .filter(|edge| edge.slot == id && state.key.is_none_or(|key| key == edge.key))
        {
            commands
                .spawn_scene(binding_row_scene(font.0.clone(), edge.key))
                .insert(ChildOf(*rows));
        }
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

fn binding_row_scene(font: Handle<Font>, key: Key) -> impl Scene {
    let label = key_label(key);
    bsn! {
        @control_button(label)
        Node { height: px(34), padding: {UiRect::horizontal(px(10))}, border: {UiRect::all(px(1))} }
        BorderColor::all(DIVIDER)
        BackgroundColor(SIDEBAR)
        on(move |_: On<Activate>, mut state: ResMut<ControlsState>| {
            start_capture(&mut state, Some(key));
        })
        Children [ @control_label(font, label, 15.0) ]
    }
}
