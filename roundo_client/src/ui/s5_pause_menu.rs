use super::{
    UiAction,
    widgets::{spawn_button, spawn_heading, spawn_label},
};
use bevy::prelude::*;

pub(super) fn spawn(commands: &mut Commands, panel: Entity, server_name: Option<&str>) {
    spawn_heading(commands, panel, "Paused");
    if let Some(server_name) = server_name {
        spawn_label(commands, panel, &format!("Connected to {server_name}"));
    }
    spawn_button(commands, panel, "Continue Game", UiAction::ContinueGame);
    spawn_button(commands, panel, "Settings", UiAction::OpenSettings);
    spawn_button(commands, panel, "Leave Game", UiAction::LeaveGameToServers);
}
