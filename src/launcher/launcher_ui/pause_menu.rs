use bevy::{
    ecs::template::FromTemplate,
    feathers::controls::FeathersButton,
    input_focus::{FocusCause, InputFocus, tab_navigation::TabGroup},
    picking::hover::Hovered,
    prelude::*,
    scene::Scene,
    text::FontSourceTemplate,
    ui_widgets::Activate,
};

use super::super::local_game::{LocalControl, LocalScreen};
use super::{
    BACKGROUND, DIVIDER, LauncherFont, MenuEmojiFont, OPTION_TITLE, OptionWindow, QUIT_EMOJI,
    SIDEBAR, close_option_window, controls::ControlsCapture, local_saves::return_to_launcher,
    menu_emoji_scene, open_option_window, primary_pointer_target_scene,
};

pub(super) const SIDEBAR_WIDTH_PX: f32 = 80.0;

pub(super) struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(LocalScreen::Game), spawn_pause_menu)
            .add_systems(
                Update,
                (
                    back_from_pause_option
                        .after(ControlsCapture)
                        .before(sync_pause_menu),
                    sync_pause_menu,
                    update_button_hover,
                )
                    .run_if(in_state(LocalScreen::Game)),
            );
    }
}

#[derive(Clone, Component, Default)]
struct PauseMenu;

#[derive(Clone, Component, Default)]
struct ResumeButton;

#[derive(Clone, Component, Default)]
struct OptionButton;

#[derive(Component, FromTemplate)]
struct PauseButtonLabel(Entity);

fn spawn_pause_menu(
    mut commands: Commands,
    font: Res<LauncherFont>,
    emoji_font: Res<MenuEmojiFont>,
) {
    let font = font.0.clone();
    let emoji_font = emoji_font.0.clone();
    commands
        .spawn_scene(bsn! {
            PauseMenu
            GlobalZIndex(2)
            TabGroup
            Visibility::Hidden
            Pickable::IGNORE
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                top: px(0), right: px(0), width: percent(100), height: percent(100),
                justify_content: JustifyContent::FlexEnd,
            }
            Children [
                Node {
                    width: px(SIDEBAR_WIDTH_PX), flex_shrink: 0.0,
                    flex_direction: FlexDirection::Column, row_gap: px(12),
                    align_items: AlignItems::Center,
                    padding: {UiRect::vertical(px(12))},
                    border: {UiRect::left(px(1))},
                }
                BackgroundColor(SIDEBAR)
                BorderColor::all(DIVIDER)
                Children [
                    @pause_button_scene(font.clone(), emoji_font.clone(), "Resume", "▶️")
                    ResumeButton
                    on(resume_game)
                    --
                    @pause_button_scene(font.clone(), emoji_font.clone(), OPTION_TITLE, "⚙️")
                    OptionButton
                    on(open_option_window)
                    --
                    Node { flex_grow: 1.0 }
                    Pickable::IGNORE
                    --
                    @pause_button_scene(font, emoji_font, "Quit", QUIT_EMOJI)
                    on(return_to_launcher)
                ]
            ]
        })
        .insert(DespawnOnExit(LocalScreen::Game));
}

fn pause_button_scene(
    font: Handle<Font>,
    emoji_font: Handle<Font>,
    label: &'static str,
    emoji: &'static str,
) -> impl Scene {
    bsn! {
        @FeathersButton
        AccessibleLabel(label)
        Pickable::IGNORE
        PauseButtonLabel(#HoverLabel)
        Node {
            width: px(54), height: px(54), flex_shrink: 0.0,
            padding: UiRect::ZERO,
            border_radius: {BorderRadius::all(percent(50))},
        }
        Children [
            @menu_emoji_scene(emoji_font, emoji, 27.0)
            --
            @primary_pointer_target_scene()
            Node { border_radius: {BorderRadius::all(percent(50))} }
            --
            #HoverLabel
            Text({label.to_owned()})
            TextFont { font: {FontSourceTemplate::Handle(font.into())}, font_size: FontSize::Px(16.0) }
            TextLayout::no_wrap()
            Visibility::Hidden
            Pickable::IGNORE
            Node {
                position_type: PositionType::Absolute, right: percent(100),
                margin: {UiRect::right(px(16))}, padding: {UiRect::axes(px(10), px(5))},
                border_radius: {BorderRadius::all(px(6))},
            }
            BackgroundColor(BACKGROUND)
        ]
    }
}

fn sync_pause_menu(
    control: Res<LocalControl>,
    mut menu: Single<(&mut Node, &mut Visibility), (With<PauseMenu>, Without<OptionWindow>)>,
    option: Single<(&mut Node, &mut Visibility), With<OptionWindow>>,
    resume: Single<Entity, With<ResumeButton>>,
    mut focus: ResMut<InputFocus>,
) {
    if !control.is_changed() {
        return;
    }
    match *control {
        LocalControl::Loading => {}
        LocalControl::Menu => {
            menu.0.display = Display::Flex;
            *menu.1 = Visibility::Inherited;
            focus.set(*resume, FocusCause::Auto);
        }
        LocalControl::Playing => {
            menu.0.display = Display::None;
            *menu.1 = Visibility::Hidden;
            close_option_window(option);
            focus.clear();
        }
    }
}

fn update_button_hover(
    mut buttons: Query<(&Hovered, &PauseButtonLabel, &mut UiTransform), Changed<Hovered>>,
    mut labels: Query<&mut Visibility>,
) {
    for (hovered, label, mut transform) in &mut buttons {
        let (scale, visibility) = if hovered.0 {
            (1.08, Visibility::Inherited)
        } else {
            (1.0, Visibility::Hidden)
        };
        transform.scale = Vec2::splat(scale);
        *labels.get_mut(label.0).expect("pause button hover label") = visibility;
    }
}

fn resume_game(_: On<Activate>, mut control: ResMut<LocalControl>, mut focus: ResMut<InputFocus>) {
    if *control == LocalControl::Menu {
        *control = LocalControl::Playing;
        focus.clear();
    }
}

fn back_from_pause_option(
    control: Res<LocalControl>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    option: Single<(&mut Node, &mut Visibility), With<OptionWindow>>,
    option_button: Single<Entity, With<OptionButton>>,
    mut focus: ResMut<InputFocus>,
) {
    if *control == LocalControl::Menu
        && option.0.display != Display::None
        && (keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right))
    {
        close_option_window(option);
        focus.set(*option_button, FocusCause::Auto);
    }
}
