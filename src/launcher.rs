use bevy::{
    app::AppExit,
    audio::AudioPlugin,
    feathers::{
        FeathersPlugins,
        controls::FeathersButton,
        dark_theme::create_dark_theme,
        display::{caption, label, label_dim},
        theme::{ThemeBackgroundColor, UiTheme},
        tokens,
    },
    input_focus::tab_navigation::TabGroup,
    log::LogPlugin,
    prelude::*,
    ui_widgets::Activate,
};

use crate::app_process::AppProcess;

pub(crate) struct LaunchTarget {
    pub(crate) button_label: &'static str,
    pub(crate) process: AppProcess,
}

pub(crate) fn run(targets: &'static [LaunchTarget]) -> AppExit {
    App::new().add_plugins(LauncherPlugin { targets }).run()
}

struct LauncherPlugin {
    targets: &'static [LaunchTarget],
}

impl Plugin for LauncherPlugin {
    fn build(&self, app: &mut App) {
        let targets = self.targets;
        app.add_plugins((
            LogPlugin::default(),
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Roundo Launcher".into(),
                        ..default()
                    }),
                    ..default()
                })
                .disable::<LogPlugin>()
                .disable::<AudioPlugin>(),
            FeathersPlugins,
        ))
        .insert_resource(UiTheme(create_dark_theme()))
        .add_systems(Startup, move |world: &mut World| create_ui(world, targets));
    }
}

#[derive(Component)]
struct LaunchButton {
    process: &'static AppProcess,
    error_label: Entity,
}

fn create_ui(world: &mut World, targets: &'static [LaunchTarget]) {
    world.spawn(Camera2d);
    let root = world
        .spawn_scene(bsn! {
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: rem(1.0),
            }
            ThemeBackgroundColor(tokens::WINDOW_BG)
            TabGroup::default()
            Children [
                @label("Roundo Launcher")
                --
                @label_dim("Choose an app to start in a separate process.")
            ]
        })
        .expect("install launcher UI")
        .id();
    for target in targets {
        let button_label = target.button_label;
        let panel = world
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: rem(0.25),
                    ..default()
                },
                ChildOf(root),
            ))
            .id();
        let button = world
            .spawn_scene(bsn! { @FeathersButton { @caption: bsn! { @caption(button_label) } } })
            .expect("install launcher start button")
            .insert(ChildOf(panel))
            .id();
        let error_label = world
            .spawn_scene(label_dim(""))
            .expect("install launcher error label")
            .insert(ChildOf(panel))
            .id();
        world
            .entity_mut(button)
            .insert(LaunchButton {
                process: &target.process,
                error_label,
            })
            .observe(start_process);
    }
}

fn start_process(event: On<Activate>, buttons: Query<&LaunchButton>, mut labels: Query<&mut Text>) {
    let button = buttons.get(event.entity).expect("launcher button");
    let message = match button.process.spawn() {
        Ok(()) => String::new(),
        Err(error) => format!("Could not start {}: {error}", button.process.id),
    };
    labels
        .get_mut(button.error_label)
        .expect("launcher error label")
        .0 = message;
}
