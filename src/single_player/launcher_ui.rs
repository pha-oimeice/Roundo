//! Native launcher scene and in-place panel navigation.

use bevy::{
    app::AppExit,
    ecs::template::{EntityTemplate, FromTemplate, OptionTemplate},
    feathers::{
        FeathersPlugins,
        controls::{
            ButtonVariant, FeathersButton, FeathersDialog, FeathersDialogFooter, FeathersScrollbar,
        },
        dark_theme::create_dark_theme,
        display::{caption, label as feathers_label},
        focus::FocusIndicator,
        font_styles::InheritableFont,
        theme::UiTheme,
    },
    input_focus::{AutoFocus, FocusCause, InputFocus, tab_navigation::TabGroup},
    math::Rot2,
    prelude::*,
    scene::{Scene, SceneList},
    text::FontSourceTemplate,
    ui_widgets::{
        Activate, ControlOrientation, RequestClose, ScrollArea, SelectedTab, Tab, TabActivation,
        TabList, ValueChange,
    },
};
use roundo_infrastructure::user_config::ClientSettingsConfig;

use super::{bundled_assets_root, config_store::ClientConfigStore};

mod directory;
mod local_saves;
mod remote_servers;

pub(crate) struct LauncherUiPlugin;

#[derive(Resource)]
struct LauncherFont(Handle<Font>);

impl Plugin for LauncherUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            FeathersPlugins,
            local_saves::LocalSavesPlugin,
            remote_servers::RemoteDirectoryPlugin,
        ))
        .insert_resource(UiTheme(create_dark_theme()))
        .add_systems(PostStartup, spawn_launcher_main_menu);
    }
}

#[derive(Clone, Copy, Component, Default, PartialEq, Eq)]
enum Panel {
    #[default]
    Start,
    Option,
}

impl Panel {
    fn menu_title(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Option => "Option",
        }
    }

    fn display_when_selected(self, selected: Self) -> Display {
        if self == selected {
            Display::Flex
        } else {
            Display::None
        }
    }
}

#[derive(Clone, Component, Default)]
struct PanelButton(Panel);

#[derive(Component, FromTemplate)]
struct LauncherTabBody(Entity);

#[derive(Clone, Component, Default)]
struct LauncherRoot;

#[derive(Clone, Component, Default)]
struct QuitButton;

#[derive(Clone, Component, Default)]
struct QuitDialog;

const BACKGROUND: Color = Color::srgb_u8(16, 21, 29);
const SIDEBAR: Color = Color::srgb_u8(24, 32, 43);
const DIVIDER: Color = Color::srgb_u8(58, 72, 89);
const QUIT_RED: Color = Color::srgb_u8(241, 82, 88);
const ACTIVE_TAB_BACKGROUND: Color = Color::srgb_u8(48, 66, 86);

const FONT_DATA_PATH: &str = "fonts/NotoSansSC.ttf";
const ROUNDO_TITLE_FONT_SIZE_PX: f32 = 60.0;
const NAVIGATION_BUTTON_FONT_SIZE_PX: f32 = 24.0;
const LAUNCHER_TAB_FONT_SIZE_PX: f32 = 20.0;

fn spawn_launcher_main_menu(world: &mut World) {
    let font_path = bundled_assets_root().join(FONT_DATA_PATH);
    let font_data = std::fs::read(&font_path).unwrap_or_else(|error| {
        panic!(
            "failed to read launcher font {}: {error}",
            font_path.display()
        )
    });
    let font = world
        .resource_mut::<Assets<Font>>()
        .add(Font::from_bytes(font_data));
    world.insert_resource(LauncherFont(font.clone()));
    let start = launcher_start_tabs_scene(&font);
    let options =
        launcher_options_tabs_scene(world.resource::<ClientConfigStore>().settings(), &font);
    world
        .spawn_scene(bsn! {
            #Launcher
            LauncherRoot
            Node { width: percent(100), height: percent(100), padding: {UiRect::all(px(24))} }
            BackgroundColor(BACKGROUND)
            TabGroup
            Children [
                Node { width: percent(100), height: percent(100), padding: {UiRect::all(px(1))}, column_gap: px(1) }
                BackgroundColor(DIVIDER)
                Children [
                    @launcher_content_slot_scene(start, options)
                    --
                    @launcher_navigation_sidebar_scene(font.clone())
                ]
                --
                @quit_confirmation_button_scene(font)
            ]
        })
        .expect("spawn launcher scene");
}

fn launcher_content_slot_scene(start: impl Scene, options: impl Scene) -> impl Scene {
    let initial = Panel::default();
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
        Children [
            @switchable_content_panel_scene(Panel::Start, initial)
            Children [ @{start} ]
            --
            @switchable_content_panel_scene(Panel::Option, initial)
            Children [ @{options} ]
        ]
    }
}

fn switchable_content_panel_scene(panel: Panel, initial: Panel) -> impl Scene {
    let visibility = if panel == initial {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    bsn! {
        panel
        visibility
        Node {
            display: {panel.display_when_selected(initial)},
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: px(0),
        }
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

fn launcher_start_tabs_scene(font: &Handle<Font>) -> impl Scene + use<> {
    let local = local_saves::local_save_list_scene(font.clone());
    let remote = remote_servers::remote_server_list_scene(font.clone());
    let font = font.clone();
    bsn! {
        @launcher_tabbed_content_layout_scene()
        Children [
            @launcher_tab_list_scene(#LocalTab)
            Children [
                #LocalTab
                @launcher_tab_header_scene(font.clone(), "Local", true, #LocalDirectory)
                --
                @launcher_tab_header_scene(font, "Remote", false, #RemoteDirectory)
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
        Node { column_gap: px(8), row_gap: px(8), flex_shrink: 0.0, flex_wrap: FlexWrap::Wrap }
        TabList { activation: TabActivation::Automatic }
        SelectedTab({OptionTemplate::Some(initial_tab)})
        on(show_selected_launcher_tab_body)
    }
}

fn launcher_tab_header_scene(
    font: Handle<Font>,
    title: &'static str,
    initially_selected: bool,
    body: EntityTemplate,
) -> impl Scene {
    let background = if initially_selected {
        ACTIVE_TAB_BACKGROUND
    } else {
        SIDEBAR
    };
    bsn! {
        Tab
        LauncherTabBody(body)
        FocusIndicator
        Node {
            min_width: px(88),
            height: px(42),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: {UiRect::horizontal(px(12))},
        }
        BackgroundColor(background)
        Children [
            @launcher_label_with_font(font, title, LAUNCHER_TAB_FONT_SIZE_PX)
            Pickable::IGNORE
        ]
    }
}

fn show_selected_launcher_tab_body(
    event: On<ValueChange<Option<Entity>>>,
    mut tab_lists: Query<(&Children, &mut SelectedTab), With<TabList>>,
    mut tabs: Query<(&LauncherTabBody, &mut BackgroundColor), With<Tab>>,
    mut bodies: Query<(&mut Node, &mut Visibility), Without<Tab>>,
) {
    let (children, mut selection) = tab_lists.get_mut(event.source).expect("launcher tab list");
    let selected = event
        .value
        .filter(|tab| children.contains(tab) && tabs.contains(*tab))
        .expect("selection must name a tab in this launcher tab list");
    selection.0 = Some(selected);
    for tab in children.iter() {
        let (body, mut background) = tabs.get_mut(tab).expect("launcher tab header");
        let (mut node, mut visibility) = bodies.get_mut(body.0).expect("launcher tab body");
        if tab == selected {
            background.0 = ACTIVE_TAB_BACKGROUND;
            node.display = Display::Flex;
            *visibility = Visibility::Inherited;
        } else {
            background.0 = SIDEBAR;
            node.display = Display::None;
            *visibility = Visibility::Hidden;
        }
    }
}

fn launcher_options_tabs_scene(
    settings: &ClientSettingsConfig,
    font: &Handle<Font>,
) -> impl Scene + use<> {
    let control = read_only_control_settings_scene(settings, font);
    let lod = settings.world.geometry_lod_pixel_error;
    let font = font.clone();
    bsn! {
        @launcher_tabbed_content_layout_scene()
        Children [
            @launcher_tab_list_scene(#GeneralTab)
            Children [
                #GeneralTab
                @launcher_tab_header_scene(font.clone(), "General", true, #GeneralSettings)
                --
                @launcher_tab_header_scene(font.clone(), "Display", false, #DisplaySettings)
                --
                @launcher_tab_header_scene(font.clone(), "Graphic", false, #GraphicSettings)
                --
                @launcher_tab_header_scene(font.clone(), "Audio", false, #AudioSettings)
                --
                @launcher_tab_header_scene(font.clone(), "Control", false, #ControlSettings)
                --
                @launcher_tab_header_scene(font.clone(), "Misc", false, #MiscSettings)
            ]
            --
            @content_header_divider_scene()
            --
            #GeneralSettings
            @launcher_label_with_font(font.clone(), "暂无通用设置。", 18.0)
            --
            #DisplaySettings
            @launcher_label_with_font(font.clone(), "暂无显示设置。", 18.0)
            Node { display: Display::None }
            Visibility::Hidden
            --
            #GraphicSettings
            @launcher_label_with_font(font.clone(), {format!("几何 LOD 像素误差（只读）：{lod}")}, 18.0)
            Node { display: Display::None }
            Visibility::Hidden
            --
            #AudioSettings
            @launcher_label_with_font(font.clone(), "暂无音频设置。", 18.0)
            Node { display: Display::None }
            Visibility::Hidden
            --
            #ControlSettings
            @{control}
            Node { display: Display::None }
            Visibility::Hidden
            --
            #MiscSettings
            @launcher_label_with_font(font, "暂无其他设置。", 18.0)
            Node { display: Display::None }
            Visibility::Hidden
        ]
    }
}

fn read_only_control_settings_scene(
    settings: &ClientSettingsConfig,
    font: &Handle<Font>,
) -> impl Scene + use<> {
    let font = font.clone();
    let bindings = settings
        .input_bindings
        .iter()
        .map(|binding| {
            launcher_label_with_font(
                font.clone(),
                format!("{}: {:?}", binding.slot, binding.key),
                18.0,
            )
        })
        .collect::<Vec<_>>();
    let contents = bsn_list! {
        @launcher_label_with_font(font, "已配置的按键绑定（只读，未启用）", 22.0)
        --
        {bindings}
    };
    launcher_scroll_area_scene(contents)
}

fn launcher_scroll_area_scene(contents: impl SceneList) -> impl Scene {
    bsn! {
        #LauncherScroll
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: px(0),
            overflow: Overflow::scroll_y(),
            row_gap: px(12),
        }
        ScrollArea
        Children [
            @FeathersScrollbar { @target: #LauncherScroll, @orientation: ControlOrientation::Vertical }
            Node { position_type: PositionType::Absolute, right: px(0), top: px(0), height: percent(100), width: px(8) }
            --
            {contents}
        ]
    }
}

fn launcher_navigation_sidebar_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(23),
            min_width: px(208),
            flex_shrink: 0.0,
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
                @panel_navigation_button_scene(Panel::Start)
                --
                @panel_navigation_button_scene(Panel::Option)
            ]
        ]
    }
}

fn panel_navigation_button_scene(panel: Panel) -> impl Scene {
    let title = panel.menu_title();
    bsn! {
        @FeathersButton { @caption: bsn! { @caption(title) } }
        InheritableFont { font_size: FontSize::Px(NAVIGATION_BUTTON_FONT_SIZE_PX) }
        Node { width: percent(100), height: px(48) }
        PanelButton(panel)
        on(show_content_panel_for_navigation_button)
    }
}

fn quit_confirmation_button_scene(font: Handle<Font>) -> impl Scene {
    bsn! {
        #Quit
        QuitButton
        @FeathersButton {
            @caption: bsn! {
                Node { width: px(22), height: px(22), align_items: AlignItems::Center, justify_content: JustifyContent::Center }
                Pickable::IGNORE
                Children [
                    @quit_icon_diagonal_stroke_scene(45.0)
                    --
                    @quit_icon_diagonal_stroke_scene(-45.0)
                ]
            }
        }
        Node { position_type: PositionType::Absolute, right: px(40), bottom: px(40), width: px(44), height: px(44) }
        on(move |_: On<Activate>, mut commands: Commands, root: Single<Entity, With<LauncherRoot>>, dialogs: Query<(), With<QuitDialog>>| {
            if dialogs.is_empty() {
                commands.spawn_scene(quit_confirmation_dialog_scene(*root, font.clone()));
            }
        })
    }
}

fn quit_icon_diagonal_stroke_scene(angle: f32) -> impl Scene {
    let transform = UiTransform::from_rotation(Rot2::degrees(angle));
    bsn! {
        Node { position_type: PositionType::Absolute, width: px(20), height: px(4), border_radius: {BorderRadius::all(px(2))} }
        transform
        BackgroundColor(QUIT_RED)
        Pickable::IGNORE
    }
}

fn quit_confirmation_dialog_scene(parent: Entity, font: Handle<Font>) -> impl Scene {
    let confirm_font = font.clone();
    let contents = bsn_list! {
        Node { flex_direction: FlexDirection::Column, padding: {UiRect::all(px(12))} }
        Children [ @launcher_label_with_font(font.clone(), "确定要退出 Roundo 吗？", 22.0) ]
        --
        @FeathersDialogFooter
        Children [
            @FeathersButton { @caption: bsn! { @caption("取消") } }
            InheritableFont { font: {font.clone()}, font_size: FontSize::Px(16.0) }
            AutoFocus
            on(request_quit_dialog_close_on_cancel)
            --
            @FeathersButton { @caption: bsn! { @caption("确认退出") }, @variant: ButtonVariant::Primary }
            InheritableFont { font: confirm_font, font_size: FontSize::Px(16.0) }
            on(exit_application_on_quit_confirmation)
        ]
    };
    let contents: Box<dyn SceneList> = Box::new(contents);
    bsn! {
        @FeathersDialog { @contents: contents, @width: px(360) }
        QuitDialog
        ChildOf(parent)
        BackgroundColor({Color::srgba(0.0, 0.0, 0.0, 0.6)})
        on(dismiss_quit_dialog_and_restore_button_focus)
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
    dialog: Single<Entity, With<QuitDialog>>,
    button: Single<Entity, With<QuitButton>>,
) {
    event.propagate(false);
    commands.entity(*dialog).try_despawn();
    focus.set(*button, FocusCause::Auto);
}

fn exit_application_on_quit_confirmation(_: On<Activate>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}

fn show_content_panel_for_navigation_button(
    event: On<Activate>,
    buttons: Query<&PanelButton>,
    mut panels: Query<(&Panel, &mut Node, &mut Visibility)>,
) {
    let selected = buttons
        .get(event.entity)
        .expect("launcher navigation button")
        .0;
    for (panel, mut node, mut visibility) in &mut panels {
        node.display = panel.display_when_selected(selected);
        *visibility = if *panel == selected {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
