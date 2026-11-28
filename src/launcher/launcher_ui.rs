//! Native launcher scene and in-place panel navigation.

use bevy::{
    app::AppExit,
    camera::visibility::VisibilitySystems,
    ecs::{
        message::MessageCursor,
        template::{EntityTemplate, FromTemplate, OptionTemplate},
    },
    feathers::{
        FeathersPlugins,
        controls::{
            ButtonVariant, FeathersButton, FeathersDialog, FeathersDialogFooter, FeathersScrollbar,
            FeathersSlider,
        },
        dark_theme::create_dark_theme,
        display::{caption, label as feathers_label},
        focus::FocusIndicator,
        font_styles::InheritableFont,
        theme::UiTheme,
    },
    input_focus::{
        AutoFocus, FocusCause, FocusGained, InputFocus, InputFocusSystems, tab_navigation::TabGroup,
    },
    picking::{
        PickingSystems,
        events::{PointerCancel, PointerClick, PointerDragEnd, PointerPress, PointerRelease},
        pointer::{PointerAction, PointerButton, PointerId, PointerInput},
    },
    prelude::*,
    scene::{Scene, SceneList},
    text::FontSourceTemplate,
    ui_widgets::{
        Activate, ControlOrientation, RequestClose, ScrollArea, ScrollIntoView, SelectedTab,
        SliderPrecision, SliderValue, Tab, TabActivation, TabList, ValueChange,
    },
};
use roundo_infrastructure::user_config::{
    ClientSettingsConfig, MAX_CHUNK_VIEW_DISTANCE, MIN_CHUNK_VIEW_DISTANCE,
};

use super::{
    bundled_assets_root,
    config_store::ClientConfigStore,
    local_game::{LocalControl, LocalScreen},
};

mod controls;
mod directory;
mod form;
mod highlight;
mod local_saves;
mod pause_menu;
mod remote_servers;
mod search;

pub(crate) struct LauncherUiPlugin;

#[derive(Resource)]
struct LauncherFont(Handle<Font>);

#[derive(Resource)]
struct MenuEmojiFont(Handle<Font>);

impl Plugin for LauncherUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            FeathersPlugins,
            controls::ControlsPlugin,
            local_saves::LocalSavesPlugin,
            pause_menu::PauseMenuPlugin,
            remote_servers::RemoteDirectoryPlugin,
        ))
        .insert_resource(UiTheme(create_dark_theme()))
        .add_observer(highlight::preserve_button_cursor)
        .add_systems(PostStartup, spawn_launcher_main_menu)
        .add_systems(
            First,
            reserve_secondary_click.in_set(PickingSystems::PostInput),
        )
        .add_systems(
            Update,
            back_from_launcher_panel
                .after(controls::ControlsCapture)
                .run_if(in_state(LocalScreen::Launcher)),
        )
        .add_systems(OnExit(LocalScreen::Game), close_option_window)
        .add_systems(
            PostUpdate,
            (
                reset_hidden_option_tabs.after(VisibilitySystems::VisibilityPropagate),
                // Bevy derives each tab's Selected marker before focus change events.
                highlight::paint_highlights.after(InputFocusSystems::FocusChangeEvents),
            ),
        );
    }
}

#[derive(Clone, Component, Default)]
struct LauncherStartPanel;

#[derive(Clone, Component, Default)]
struct LauncherStartButton;

#[derive(Clone, Component, Default)]
struct LauncherOptionButton;

#[derive(Clone, Component, Default)]
struct OptionWindow;

#[derive(Clone, Component, Default)]
struct RenderRadiusStatus;

#[derive(Component, FromTemplate)]
struct LauncherTabBody(Entity);

#[derive(Component, FromTemplate)]
struct OptionTabList(Entity);

#[derive(Clone, Component, Default)]
struct LauncherRoot;

#[derive(Component, FromTemplate)]
struct QuitDialog(Entity);

const BACKGROUND: Color = Color::srgb_u8(16, 21, 29);
const SIDEBAR: Color = Color::srgb_u8(24, 32, 43);
const DIVIDER: Color = Color::srgb_u8(58, 72, 89);
const OPTION_TITLE: &str = "Option";
const QUIT_EMOJI: &str = "🚪";

const FONT_DATA_PATH: &str = "fonts/NotoSansSC.ttf";
const ROUNDO_TITLE_FONT_SIZE_PX: f32 = 60.0;
const NAVIGATION_BUTTON_FONT_SIZE_PX: f32 = 24.0;
const LAUNCHER_TAB_FONT_SIZE_PX: f32 = 20.0;

fn load_bundled_font(world: &mut World, path: &str) -> Handle<Font> {
    let font_path = bundled_assets_root().join(path);
    let font_data = std::fs::read(&font_path).unwrap_or_else(|error| {
        panic!(
            "failed to read bundled font {}: {error}",
            font_path.display()
        )
    });
    world
        .resource_mut::<Assets<Font>>()
        .add(Font::from_bytes(font_data))
}

fn spawn_launcher_main_menu(world: &mut World) {
    let font = load_bundled_font(world, FONT_DATA_PATH);
    world.insert_resource(LauncherFont(font.clone()));
    let emoji_font = load_bundled_font(world, "fonts/NotoEmoji-Menu.ttf");
    world.insert_resource(MenuEmojiFont(emoji_font.clone()));
    let start = launcher_start_tabs_scene(&font, &emoji_font);
    let options = option_window_scene(
        world.resource::<ClientConfigStore>().settings(),
        &font,
        &emoji_font,
    );
    world
        .spawn_scene(bsn! {
            @launcher_root_layout_scene()
            LauncherRoot
            BackgroundColor(BACKGROUND)
            TabGroup
            Children [
                @launcher_frame_layout_scene()
                BackgroundColor(DIVIDER)
                Children [
                    @launcher_content_slot_scene()
                    Children [
                        LauncherStartPanel
                        Node { display: Display::None, flex_direction: FlexDirection::Column, flex_grow: 1.0, min_height: px(0) }
                        Visibility::Hidden
                        Children [ @{start} ]
                    ]
                    --
                    @launcher_navigation_sidebar_scene(font.clone())
                ]
                --
                @quit_confirmation_button_scene(font, emoji_font)
            ]
        })
        .expect("spawn launcher scene");
    world
        .spawn_scene(options)
        .expect("spawn shared Option window");
}

fn launcher_root_layout_scene() -> impl Scene {
    bsn! { Node { width: percent(100), height: percent(100), padding: {UiRect::all(px(24))} } }
}

fn launcher_frame_layout_scene() -> impl Scene {
    bsn! { Node { width: percent(100), height: percent(100), padding: {UiRect::all(px(1))}, column_gap: px(1) } }
}

fn launcher_content_slot_scene() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: px(0),
            min_width: px(0),
            padding: {UiRect::all(px(24))},
            row_gap: px(16),
        }
        BackgroundColor(BACKGROUND)
    }
}

fn launcher_label_with_font(font: Handle<Font>, text: impl Into<String>, size: f32) -> impl Scene {
    bsn! {
        @feathers_label(text)
        TextFont { font: {FontSourceTemplate::Handle(font.into())}, font_size: FontSize::Px(size) }
    }
}

fn content_header_divider_scene() -> impl Scene {
    bsn! {
        Node { width: percent(100), height: px(1), flex_shrink: 0.0 }
        BackgroundColor(DIVIDER)
    }
}

fn launcher_start_tabs_scene(font: &Handle<Font>, emoji_font: &Handle<Font>) -> impl Scene + use<> {
    let local = local_saves::local_save_list_scene(font.clone(), emoji_font.clone());
    let remote = remote_servers::remote_server_list_scene(font.clone(), emoji_font.clone());
    let font = font.clone();
    bsn! {
        @launcher_tabbed_content_layout_scene()
        Children [
            @launcher_tab_list_scene(#LocalTab)
            Children [
                #LocalTab
                @launcher_tab_header_scene(font.clone(), "Local", #LocalDirectory)
                --
                @launcher_tab_header_scene(font, "Remote", #RemoteDirectory)
            ]
            --
            @content_header_divider_scene()
            --
            #LocalDirectory
            @{local}
            --
            #RemoteDirectory
            @{remote}
            Node { display: Display::None }
            Visibility::Hidden
        ]
    }
}

fn launcher_tabbed_content_layout_scene() -> impl Scene {
    bsn! {
        Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, min_height: px(0), row_gap: px(12) }
    }
}

fn launcher_tab_list_scene(initial_tab: EntityTemplate) -> impl Scene {
    bsn! {
        Node { column_gap: px(8), flex_shrink: 0.0 }
        TabList { activation: TabActivation::Automatic }
        SelectedTab({OptionTemplate::Some(initial_tab)})
        on(show_selected_launcher_tab_body)
    }
}

fn launcher_tab_header_scene(
    font: Handle<Font>,
    title: &'static str,
    body: EntityTemplate,
) -> impl Scene {
    bsn! {
        Tab
        LauncherTabBody(body)
        FocusIndicator
        highlight::Highlight
        Node {
            min_width: px(88),
            height: px(42), flex_shrink: 0.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: {UiRect::horizontal(px(12))},
        }
        Children [
            @launcher_label_with_font(font, title, LAUNCHER_TAB_FONT_SIZE_PX)
            Pickable::IGNORE
        ]
    }
}

fn show_selected_launcher_tab_body(
    event: On<ValueChange<Option<Entity>>>,
    mut tab_lists: Query<(&Children, &mut SelectedTab), With<TabList>>,
    tabs: Query<&LauncherTabBody, With<Tab>>,
    mut bodies: Query<(&mut Node, &mut Visibility), Without<Tab>>,
) {
    let (children, mut selection) = tab_lists.get_mut(event.source).expect("launcher tab list");
    let selected = event
        .value
        .filter(|tab| children.contains(tab) && tabs.contains(*tab))
        .expect("selection must name a tab in this launcher tab list");
    if selection.0 == Some(selected) {
        return;
    }
    selection.0 = Some(selected);
    for tab in children.iter() {
        let body = tabs.get(tab).expect("launcher tab header");
        let (mut node, mut visibility) = bodies.get_mut(body.0).expect("launcher tab body");
        if tab == selected {
            node.display = Display::Flex;
            *visibility = Visibility::Inherited;
        } else {
            node.display = Display::None;
            *visibility = Visibility::Hidden;
        }
    }
}

fn option_window_scene(
    settings: &ClientSettingsConfig,
    font: &Handle<Font>,
    emoji: &Handle<Font>,
) -> impl Scene + use<> {
    let tabs = option_tabs_scene(settings, font, emoji);
    bsn! {
        @launcher_root_layout_scene()
        Node { position_type: PositionType::Absolute, top: px(0), left: px(0) }
        Pickable::IGNORE
        TabGroup
        Children [
            @launcher_frame_layout_scene()
            OptionWindow
            GlobalZIndex(1)
            Node { display: Display::None }
            Visibility::Hidden
            Pickable::IGNORE
            Children [
                @launcher_content_slot_scene()
                Children [ @{tabs} ]
                --
                @launcher_sidebar_size_scene()
                Pickable::IGNORE
            ]
        ]
    }
}

fn option_tabs_scene(
    settings: &ClientSettingsConfig,
    font: &Handle<Font>,
    emoji: &Handle<Font>,
) -> impl Scene + use<> {
    let [general, display, graphics, audio, controls, misc] =
        option_tab_bodies(settings, font, emoji);
    let font = font.clone();
    bsn! {
        @launcher_tabbed_content_layout_scene()
        Children [
            Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0, padding: {UiRect::bottom(px(10))} }
            Children [
                #OptionTabList
                @launcher_tab_list_scene(#GeneralTab)
                OptionTabList(#GeneralTab)
                Node { min_width: px(0), overflow: {Overflow { y: OverflowAxis::Clip, ..Overflow::scroll_x() }} }
                ScrollArea
                on(reveal_focused_tab)
                Children [
                    #GeneralTab
                    @launcher_tab_header_scene(font.clone(), "General", #GeneralSettings)
                    --
                    @launcher_tab_header_scene(font.clone(), "Display", #DisplaySettings)
                    --
                    @launcher_tab_header_scene(font.clone(), "Graphics", #GraphicSettings)
                    --
                    @launcher_tab_header_scene(font.clone(), "Audio", #AudioSettings)
                    --
                    @launcher_tab_header_scene(font.clone(), "Controls", #ControlSettings)
                    --
                    @launcher_tab_header_scene(font, "Misc", #MiscSettings)
                ]
                --
                @FeathersScrollbar { @target: #OptionTabList, @orientation: ControlOrientation::Horizontal }
                Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(0), height: px(8) }
            ]
            --
            @content_header_divider_scene()
            --
            #GeneralSettings @{general}
            --
            #DisplaySettings @{display}
            --
            #GraphicSettings @{graphics}
            --
            #AudioSettings @{audio}
            --
            #ControlSettings @{controls}
            --
            #MiscSettings @{misc}
        ]
    }
}

fn option_tab_bodies(
    settings: &ClientSettingsConfig,
    font: &Handle<Font>,
    emoji: &Handle<Font>,
) -> [impl Scene + use<>; 6] {
    let label = |text: String| -> Box<dyn Scene> {
        Box::new(launcher_label_with_font(font.clone(), text, 18.0))
    };
    let graphics: Box<dyn Scene> = Box::new(graphics_settings_scene(settings, font.clone()));
    let controls: Box<dyn Scene> = Box::new(controls::controls_scene(
        settings,
        font.clone(),
        emoji.clone(),
    ));
    let contents = [
        (true, label("No general settings available.".into())),
        (false, label("No display settings available.".into())),
        (false, graphics),
        (false, label("No audio settings available.".into())),
        (false, controls),
        (false, label("No other settings available.".into())),
    ];
    contents.map(|(initially_selected, content)| {
        let (display, visibility) = if initially_selected {
            (Display::Flex, Visibility::Inherited)
        } else {
            (Display::None, Visibility::Hidden)
        };
        bsn! {
            Node { display, flex_direction: FlexDirection::Column, flex_grow: 1.0, min_height: px(0) }
            visibility
            Children [ @{content} ]
        }
    })
}

fn graphics_settings_scene(
    settings: &ClientSettingsConfig,
    font: Handle<Font>,
) -> impl Scene + use<> {
    let radius = settings.world.chunk_view_distance;
    let lod = settings.world.geometry_lod_pixel_error;
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(8) }
        Children [
            @launcher_label_with_font(font.clone(), "Render radius (world units)", 18.0)
            --
            @FeathersSlider {
                @min: MIN_CHUNK_VIEW_DISTANCE,
                @max: MAX_CHUNK_VIEW_DISTANCE,
            }
            SliderValue(radius)
            SliderPrecision::default()
            on(change_render_radius)
            on(cancel_render_radius_drag)
            --
            @launcher_label_with_font(font.clone(), "", 18.0)
            RenderRadiusStatus
            --
            @launcher_label_with_font(font, format!("Geometry LOD pixel error (read-only): {lod}"), 18.0)
        ]
    }
}

fn change_render_radius(
    event: On<ValueChange<f32>>,
    mut commands: Commands,
    mut config: ResMut<ClientConfigStore>,
    mut status: Single<&mut Text, With<RenderRadiusStatus>>,
) {
    let radius = if event.is_final {
        status.0 = config
            .edit_chunk_view_distance(event.value)
            .err()
            .unwrap_or_default();
        config.settings().world.chunk_view_distance
    } else {
        event.value
    };
    commands.entity(event.source).insert(SliderValue(radius));
}

fn cancel_render_radius_drag(
    event: On<PointerCancel>,
    config: Res<ClientConfigStore>,
    mut commands: Commands,
) {
    commands
        .entity(event.entity)
        .insert(SliderValue(config.settings().world.chunk_view_distance));
}

fn reveal_focused_tab(event: On<FocusGained>, mut commands: Commands) {
    if event.cause == FocusCause::Navigated {
        commands.trigger(ScrollIntoView {
            entity: event.entity,
        });
    }
}

fn reset_hidden_option_tabs(
    mut lists: Query<
        (
            Entity,
            &InheritedVisibility,
            &OptionTabList,
            &SelectedTab,
            &mut ScrollPosition,
        ),
        Changed<InheritedVisibility>,
    >,
    mut commands: Commands,
) {
    for (entity, visibility, default_tab, selection, mut scroll) in &mut lists {
        if !visibility.get() {
            scroll.0 = Vec2::ZERO;
            if selection.0 != Some(default_tab.0) {
                commands.trigger(ValueChange::<Option<Entity>> {
                    source: entity,
                    value: Some(default_tab.0),
                    is_final: true,
                });
            }
        }
    }
}

fn launcher_scroll_area_scene(contents: impl SceneList) -> impl Scene {
    bsn! {
        @vertical_scroll_frame_scene()
        Children [
            #LauncherScroll
            @vertical_scroll_viewport_scene()
            Node { row_gap: px(12) }
            Children [ {contents} ]
            --
            @vertical_scrollbar_scene(#LauncherScroll)
        ]
    }
}

fn vertical_scroll_frame_scene() -> impl Scene {
    bsn! {
        Node { flex_grow: 1.0, min_width: px(0), min_height: px(0), padding: {UiRect::right(px(10))} }
    }
}

fn vertical_scroll_viewport_scene() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column, flex_grow: 1.0,
            min_width: px(0), min_height: px(0),
            overflow: {Overflow { x: OverflowAxis::Clip, ..Overflow::scroll_y() }},
        }
        ScrollArea
    }
}

fn vertical_scrollbar_scene(target: EntityTemplate) -> impl Scene {
    bsn! {
        @FeathersScrollbar { @target: target, @orientation: ControlOrientation::Vertical }
        Node { position_type: PositionType::Absolute, right: px(0), top: px(0), bottom: px(0), width: px(8) }
    }
}

fn launcher_sidebar_size_scene() -> impl Scene {
    bsn! { Node { width: percent(23), min_width: px(208), flex_shrink: 0.0 } }
}

fn launcher_navigation_sidebar_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        @launcher_sidebar_size_scene()
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding: {UiRect { bottom: px(82), ..UiRect::all(px(24)) }},
        }
        BackgroundColor(SIDEBAR)
        Children [
            Node { width: percent(100), height: percent(40), align_items: AlignItems::Center, justify_content: JustifyContent::Center }
            Children [ @launcher_label_with_font(font, "Roundo", ROUNDO_TITLE_FONT_SIZE_PX) ]
            --
            Node {
                flex_direction: FlexDirection::Column,
                width: percent(100),
                max_width: px(208),
                flex_shrink: 0.0,
                row_gap: px(12),
            }
            Children [
                @launcher_navigation_button_scene("Start")
                LauncherStartButton
                on(show_launcher_start)
                --
                @launcher_navigation_button_scene(OPTION_TITLE)
                LauncherOptionButton
                on(open_option_window)
            ]
        ]
    }
}

fn launcher_navigation_button_scene(title: &'static str) -> impl Scene {
    bsn! {
        @FeathersButton { @caption: bsn! { @caption(title) } }
        InheritableFont { font_size: FontSize::Px(NAVIGATION_BUTTON_FONT_SIZE_PX) }
        Node { width: percent(100), height: px(48) }
    }
}

// Feathers buttons accept all pointer buttons. Filter on the hit target before events reach the widget.
fn primary_pointer_target_scene() -> impl Scene {
    bsn! {
        Node { position_type: PositionType::Absolute, top: px(0), right: px(0), bottom: px(0), left: px(0) }
        on(|mut event: On<PointerPress>| {
            if event.button != PointerButton::Primary { event.propagate(false); }
        })
        on(|mut event: On<PointerRelease>| {
            if event.button != PointerButton::Primary { event.propagate(false); }
        })
        on(|mut event: On<PointerClick>| {
            if event.button != PointerButton::Primary { event.propagate(false); }
        })
        on(|mut event: On<PointerDragEnd>| {
            if event.button != PointerButton::Primary { event.propagate(false); }
        })
    }
}

fn menu_emoji_scene(font: Handle<Font>, emoji: &'static str, size: f32) -> impl Scene {
    bsn! {
        Text({emoji.to_owned()})
        TextFont { font: {FontSourceTemplate::Handle(font.into())}, font_size: FontSize::Px(size) }
        Pickable::IGNORE
    }
}

fn quit_confirmation_button_scene(font: Handle<Font>, emoji_font: Handle<Font>) -> impl Scene {
    bsn! {
        @FeathersButton
        AccessibleLabel("Quit")
        Node { position_type: PositionType::Absolute, right: px(40), bottom: px(40), width: px(44), height: px(44), padding: UiRect::ZERO }
        Children [ @menu_emoji_scene(emoji_font, QUIT_EMOJI, 24.0) ]
        on(move |event: On<Activate>, mut commands: Commands, root: Single<Entity, With<LauncherRoot>>, dialogs: Query<(), With<QuitDialog>>| {
            if dialogs.is_empty() {
                commands.spawn_scene(quit_confirmation_dialog_scene(*root, event.entity, font.clone()));
            }
        })
    }
}

fn quit_confirmation_dialog_scene(
    parent: Entity,
    opener: Entity,
    font: Handle<Font>,
) -> impl Scene {
    let confirm_font = font.clone();
    let contents = bsn_list! {
        Node { flex_direction: FlexDirection::Column, padding: {UiRect::all(px(12))} }
        Children [ @launcher_label_with_font(font.clone(), "Are you sure you want to quit Roundo?", 22.0) ]
        --
        @FeathersDialogFooter
        Children [
            @FeathersButton { @caption: bsn! { @caption("Cancel") } }
            InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
            AutoFocus
            on(request_quit_dialog_close_on_cancel)
            --
            @FeathersButton { @caption: bsn! { @caption("Quit") }, @variant: ButtonVariant::Primary }
            InheritableFont { font: confirm_font, font_size: FontSize::Px(16.0) }
            on(exit_application_on_quit_confirmation)
        ]
    };
    let contents: Box<dyn SceneList> = Box::new(contents);
    bsn! {
        @FeathersDialog { @contents: contents, @width: px(360) }
        QuitDialog(opener)
        ChildOf(parent)
        BackgroundColor({Color::srgba(0.0, 0.0, 0.0, 0.6)})
        on(dismiss_quit_dialog_and_restore_button_focus)
    }
}

fn reserve_secondary_click(
    mut pointer_inputs: ResMut<Messages<PointerInput>>,
    mut cursor: Local<MessageCursor<PointerInput>>,
) {
    let is_mouse_secondary = |input: &PointerInput| {
        input.pointer_id == PointerId::Mouse
            && matches!(
                input.action,
                PointerAction::Press(PointerButton::Secondary)
                    | PointerAction::Release(PointerButton::Secondary)
            )
    };
    let mut scan = cursor.clone();
    if scan.read(&pointer_inputs).any(is_mouse_secondary) {
        // Leave raw mouse input for Back and rebinding. Requeue only new picking input.
        let inputs: Vec<_> = cursor
            .read(&pointer_inputs)
            .filter(|input| !is_mouse_secondary(input))
            .cloned()
            .collect();
        pointer_inputs.clear();
        pointer_inputs.write_batch(inputs);
    }
    cursor.clear(&pointer_inputs);
}

fn menu_back_pressed(keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
    keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right)
}

fn back_from_launcher_panel(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    dialog: Option<Single<Entity, With<QuitDialog>>>,
    option: Single<(&mut Node, &mut Visibility), With<OptionWindow>>,
    mut start: Single<
        (&mut Node, &mut Visibility),
        (With<LauncherStartPanel>, Without<OptionWindow>),
    >,
    option_button: Single<Entity, With<LauncherOptionButton>>,
    start_button: Single<Entity, With<LauncherStartButton>>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    if !menu_back_pressed(&keys, &mouse) {
        return;
    }
    if let Some(dialog) = dialog {
        commands.trigger(RequestClose { source: *dialog });
    } else if option.0.display != Display::None {
        close_option_window(option);
        focus.set(*option_button, FocusCause::Auto);
    } else if start.0.display != Display::None {
        start.0.display = Display::None;
        *start.1 = Visibility::Hidden;
        focus.set(*start_button, FocusCause::Auto);
    }
}

fn request_quit_dialog_close_on_cancel(event: On<Activate>, mut commands: Commands) {
    commands.trigger(RequestClose {
        source: event.entity,
    });
}

fn dismiss_quit_dialog_and_restore_button_focus(
    mut event: On<RequestClose>,
    mut commands: Commands,
    mut focus: ResMut<InputFocus>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    dialog: Single<(Entity, &QuitDialog)>,
) {
    event.propagate(false);
    // Feathers can dismiss the dialog in PreUpdate; do not also back out of its panel.
    keys.clear_just_pressed(KeyCode::Escape);
    mouse.clear_just_pressed(MouseButton::Right);
    let (dialog, opener) = *dialog;
    commands.entity(dialog).try_despawn();
    focus.set(opener.0, FocusCause::Auto);
}

fn exit_application_on_quit_confirmation(_: On<Activate>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}

fn open_option_window(
    _: On<Activate>,
    screen: Res<State<LocalScreen>>,
    control: Res<LocalControl>,
    mut option: Single<(&mut Node, &mut Visibility), With<OptionWindow>>,
    mut start: Single<
        (&mut Node, &mut Visibility),
        (With<LauncherStartPanel>, Without<OptionWindow>),
    >,
) {
    if option.0.display != Display::None {
        return;
    }
    match screen.get() {
        LocalScreen::Launcher => {
            start.0.display = Display::None;
            *start.1 = Visibility::Hidden;
            option.0.flex_direction = FlexDirection::Row;
            option.0.right = Val::Auto;
        }
        LocalScreen::Game if *control == LocalControl::Menu => {
            // Move the same window beside the pause rail without changing its layout size.
            option.0.flex_direction = FlexDirection::RowReverse;
            option.0.right = px(pause_menu::SIDEBAR_WIDTH_PX);
        }
        LocalScreen::Game => return,
    }
    option.0.display = Display::Flex;
    *option.1 = Visibility::Inherited;
}

fn show_launcher_start(
    _: On<Activate>,
    option: Single<(&mut Node, &mut Visibility), With<OptionWindow>>,
    mut start: Single<
        (&mut Node, &mut Visibility),
        (With<LauncherStartPanel>, Without<OptionWindow>),
    >,
) {
    if start.0.display == Display::None {
        close_option_window(option);
        start.0.display = Display::Flex;
        *start.1 = Visibility::Inherited;
    }
}

fn close_option_window(mut option: Single<(&mut Node, &mut Visibility), With<OptionWindow>>) {
    if option.0.display != Display::None {
        option.0.display = Display::None;
        *option.1 = Visibility::Hidden;
    }
}
