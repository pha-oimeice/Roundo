use std::process::{Child, Command, Stdio};

use bevy::{
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

use crate::Role;

pub(crate) struct LauncherPlugin;

impl Plugin for LauncherPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
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
        .add_systems(Startup, create_ui)
        .add_systems(Update, poll_processes);
    }
}

#[derive(Component)]
struct ProcessSlot {
    role: Role,
    child: Option<Child>,
    status_label: Entity,
}

fn create_ui(world: &mut World) {
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
                @label_dim("Choose a role to start in a separate process.")
            ]
        })
        .expect("install launcher UI")
        .id();
    for (role, button_label) in [
        (Role::Server, "Start Server"),
        (Role::Client, "Start Client"),
    ] {
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
        let status_label = world
            .spawn_scene(label_dim("Not started"))
            .expect("install launcher status label")
            .insert(ChildOf(panel))
            .id();
        world
            .entity_mut(button)
            .insert(ProcessSlot {
                role,
                child: None,
                status_label,
            })
            .observe(start_process);
    }
}

fn start_process(
    event: On<Activate>,
    mut slots: Query<&mut ProcessSlot>,
    mut statuses: Query<&mut Text>,
) {
    let mut slot = slots.get_mut(event.entity).expect("launcher process slot");
    if slot.child.is_some() {
        return;
    }

    let argument = slot.role.argument();
    let status = match std::env::current_exe().and_then(|executable| {
        Command::new(executable)
            .arg(argument)
            .stdin(Stdio::null())
            .spawn()
    }) {
        Ok(child) => {
            let pid = child.id();
            info!("Launcher spawned {argument} (PID {pid})");
            slot.child = Some(child);
            format!("Process running (PID {pid})")
        }
        Err(error) => format!("Could not start {argument}: {error}"),
    };
    show_status(&mut statuses, slot.status_label, status);
}

fn poll_processes(mut slots: Query<&mut ProcessSlot>, mut statuses: Query<&mut Text>) {
    for mut slot in &mut slots {
        let Some(child) = &mut slot.child else {
            continue;
        };
        let status = match child.try_wait() {
            Ok(None) => continue,
            Ok(Some(status)) => {
                slot.child = None;
                format!("Process exited ({status})")
            }
            Err(error) => format!("Could not check process status: {error}"),
        };
        show_status(&mut statuses, slot.status_label, status);
    }
}

fn show_status(statuses: &mut Query<&mut Text>, label: Entity, status: String) {
    let mut text = statuses.get_mut(label).expect("launcher status label");
    if text.0 != status {
        text.0 = status;
    }
}
