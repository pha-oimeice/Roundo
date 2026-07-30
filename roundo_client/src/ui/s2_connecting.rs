use super::{
    UiAction,
    state::ClientUiState,
    widgets::{spawn_button, spawn_heading, spawn_label, spawn_status},
};
use bevy::prelude::*;

pub(super) fn spawn(commands: &mut Commands, panel: Entity, state: &ClientUiState) {
    spawn_heading(commands, panel, "Connecting");
    if let Some(server) = state.connecting_server.as_ref() {
        spawn_label(commands, panel, &server.name);
        spawn_label(commands, panel, &format!("Game: {}", server.game_addr));
        spawn_label(commands, panel, &format!("HTTPS: {}", server.https_addr));
    }
    spawn_status(commands, panel, &state.status_message);
    spawn_button(commands, panel, "Retry", UiAction::RetryConnection);
    spawn_button(commands, panel, "Cancel", UiAction::CancelConnection);
}
