use super::{
    UiAction,
    widgets::{spawn_adjustment_buttons, spawn_button, spawn_heading, spawn_label},
};
use bevy::prelude::*;
use roundo_marionette::ClientMarionetteInputSettings;

pub(super) fn spawn(
    commands: &mut Commands,
    panel: Entity,
    settings: &ClientMarionetteInputSettings,
) {
    spawn_heading(commands, panel, "Settings");
    spawn_label(
        commands,
        panel,
        &format!("Mouse sensitivity: {:.4}", settings.mouse_sensitivity),
    );
    spawn_adjustment_buttons(
        commands,
        panel,
        UiAction::SensitivityDown,
        UiAction::SensitivityUp,
    );
    spawn_label(
        commands,
        panel,
        &format!("Camera speed: {:.1}", settings.camera_move_speed),
    );
    spawn_adjustment_buttons(commands, panel, UiAction::SpeedDown, UiAction::SpeedUp);
    spawn_button(commands, panel, "Back", UiAction::CloseSettings);
}
