use super::{
    UiAction,
    widgets::{spawn_button, spawn_heading, spawn_label},
};
use bevy::prelude::*;

#[derive(Component)]
pub(super) struct IntroLogo;

pub(super) fn spawn(commands: &mut Commands, panel: Entity, intro_finished: bool) {
    if !intro_finished {
        let logo = commands
            .spawn((
                IntroLogo,
                Text::new("ROUNDO"),
                TextFont {
                    font_size: FontSize::Px(58.0),
                    ..default()
                },
                TextColor(Color::srgba(0.85, 0.92, 1.0, 0.0)),
                UiTransform::from_scale(Vec2::splat(0.88)),
            ))
            .id();
        commands.entity(panel).add_child(logo);
        return;
    }

    spawn_heading(commands, panel, "ROUNDO");
    spawn_label(commands, panel, "Main menu");
    spawn_button(commands, panel, "Start Game", UiAction::StartGame);
    spawn_button(commands, panel, "Settings", UiAction::OpenSettings);
    spawn_button(commands, panel, "Quit Game", UiAction::QuitApplication);
}
