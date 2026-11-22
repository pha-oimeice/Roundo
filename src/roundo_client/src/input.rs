//! Window-local navigation for the native client UI.

use bevy::{prelude::*, window::PrimaryWindow};
use roundo_ui::{UiLifecycleManager, UiReady, UiSystemSet};

pub(crate) struct ClientUiInputPlugin;

impl Plugin for ClientUiInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            navigate_back
                .after(UiSystemSet::Ingress)
                .before(UiSystemSet::Project)
                .run_if(resource_exists::<UiReady>),
        );
    }
}

fn navigate_back(world: &mut World, mut blocked: Local<bool>) {
    let focused = world
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .single(world)
        .is_ok_and(|window| window.focused);
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let pressed = keys.pressed(KeyCode::Escape);
    let just_pressed = keys.just_pressed(KeyCode::Escape);
    if !focused {
        *blocked |= pressed;
    }
    if !pressed && !just_pressed {
        *blocked = false;
    }
    if focused && just_pressed && !*blocked {
        UiLifecycleManager::with(world, |ui| {
            if let Some(focused) = ui.focused_instance()
                && !ui.is_root(focused)
                && let Err(error) = ui.back(focused)
            {
                log::warn!("native UI back rejected: {error}");
            }
        });
    }
}
