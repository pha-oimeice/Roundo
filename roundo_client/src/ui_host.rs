//! Client-side adapters for the Web UI seam.
//!
//! This module has no page-flow state: it applies focused-UI presentation
//! declarations supplied by the Web UI lifecycle manager, while authoritative
//! connection state alone controls game-camera activation.
use crate::targeting::ClientVoxelRaycastSettings;
use bevy::{
    app::AppExit,
    input::ButtonInput,
    prelude::{
        Camera, Camera2d, Camera3d, ClearColorConfig, Color, Commands, Component, Entity,
        IntoScheduleConfigs, KeyCode, Message, MessageReader, MessageWriter, MouseButton, Query,
        Res, ResMut, Resource, Startup, Update, With, Without,
    },
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, Window},
};
use roundo_cli::client_network::{ClientConnectionStatus, ClientNetworkManager};
use roundo_marionette::{ClientKeyBindings, ClientMarionetteInputSettings, ClientPlayerController};
use roundo_presence::ClientPresenceSettings;
use roundo_webui::{
    RecoveryAction, RecoveryActionRequest, UiCommandSource, UiLifecycleManager, UiLifecycleState,
    UiNavigationExecutor,
};

/// Requests activation or deactivation of the player-bound game camera.
/// UI-state application emits this intent; the camera adapter owns the Bevy
/// camera mutation separately.
#[derive(Clone, Copy, Debug, Message)]
struct CameraActivationMessage {
    active: bool,
}

/// The last UI state whose host declarations were applied. This makes UI
/// transition effects edge-triggered even if unrelated systems mark the UI
/// resource as changed during a frame.
#[derive(Default, Resource)]
struct AppliedUiState(Option<(Option<roundo_webui::UiInstanceId>, bool)>);

/// A full-window 2D camera which clears the render target while game rendering
/// is disabled. Keeping it active in Web UI mode prevents old world pixels
/// from surviving beneath the WebView or appearing during teardown.
#[derive(Component)]
struct UiClearCamera;

pub struct ClientWebUiHostPlugin;
impl bevy::prelude::Plugin for ClientWebUiHostPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.init_resource::<AppliedUiState>()
            .add_message::<CameraActivationMessage>()
            .add_systems(Startup, spawn_ui_clear_camera)
            .add_systems(
                Update,
                (
                    apply_recovery_action,
                    sync_authoritative_root,
                    clear_webui_focus_on_background_click,
                    apply_ui_resource,
                    apply_camera_activation,
                    escape_opens_pause,
                    sync_runtime_settings,
                )
                    .chain(),
            );
    }
}

fn apply_recovery_action(
    mut actions: MessageReader<RecoveryActionRequest>,
    mut network: ResMut<ClientNetworkManager>,
    mut manager: ResMut<UiLifecycleManager>,
    mut navigation: bevy::ecs::system::NonSendMut<UiNavigationExecutor>,
    mut app_exit: MessageWriter<AppExit>,
) {
    for request in actions.read() {
        match request.0 {
            RecoveryAction::Retry => {}
            RecoveryAction::Disconnect => {
                network.disconnect();
                if let Err(error) = navigation
                    .replace_root(&mut manager, UiLifecycleState::Disconnected)
                    .and_then(|_| navigation.open_configured_root(&mut manager).map(|_| ()))
                {
                    log::error!("cannot recover to the Disconnected Root: {error}");
                }
            }
            RecoveryAction::Quit => {
                app_exit.write(AppExit::Success);
            }
        }
    }
}

fn authoritative_lifecycle_state(
    current: UiLifecycleState,
    status: &ClientConnectionStatus,
) -> UiLifecycleState {
    match (current, status) {
        // Connecting and pre-authentication errors still belong to the
        // Disconnected Root. Only the first authoritative Connected fact
        // replaces it.
        (UiLifecycleState::Disconnected, ClientConnectionStatus::Connected) => {
            UiLifecycleState::Connected
        }
        // Reconnecting and Error do not revoke an established session root.
        // Only explicit/authoritative Disconnected replaces it.
        (UiLifecycleState::Connected, ClientConnectionStatus::Disconnected) => {
            UiLifecycleState::Disconnected
        }
        _ => current,
    }
}

fn sync_authoritative_root(
    network: Res<ClientNetworkManager>,
    mut manager: ResMut<UiLifecycleManager>,
    mut navigation: bevy::ecs::system::NonSendMut<UiNavigationExecutor>,
) {
    let connection = network.status();
    let previous = manager.lifecycle_state();
    let expected = authoritative_lifecycle_state(previous, &connection.status);
    if previous == expected {
        return;
    }
    log::info!(
        "Client UI lifecycle transition requested: {previous:?} -> {expected:?}; connection_status={:?}",
        connection.status
    );
    let destroyed = match navigation.replace_root(&mut manager, expected) {
        Ok(destroyed) => destroyed,
        Err(error) => {
            log::error!(
                "Client UI lifecycle transition failed while replacing Root: {previous:?} -> {expected:?}; error={error}"
            );
            return;
        }
    };
    log::info!(
        "Client UI Root replaced: {previous:?} -> {expected:?}; destroyed_instances={}",
        destroyed.len()
    );
    match navigation.open_configured_root(&mut manager) {
        Ok(Some(instance)) => log::info!(
            "Configured {expected:?} Root UI staged: pending_instance={}",
            instance.get()
        ),
        Ok(None) => log::info!("No configured Root UI for {expected:?}"),
        Err(error) => log::error!(
            "Client UI lifecycle transition could not stage {expected:?} Root UI: {error}"
        ),
    }
}

fn clear_webui_focus_on_background_click(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut manager: ResMut<UiLifecycleManager>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(position) = windows.iter().next().and_then(Window::cursor_position) else {
        return;
    };
    if manager
        .interactive_instance_at(position.x, position.y)
        .is_none()
    {
        manager.clear_focus();
    }
}

fn apply_ui_resource(
    state: Res<UiLifecycleManager>,
    mut applied: ResMut<AppliedUiState>,
    mut controller: ResMut<ClientPlayerController>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut camera_activation: MessageWriter<CameraActivationMessage>,
) {
    let connected = state.lifecycle_state() == UiLifecycleState::Connected;
    let focused = state.focused_declaration();
    let state_key = (focused.map(|value| value.instance), connected);
    if applied.0 == Some(state_key) {
        return;
    }
    let game_input = connected && focused.is_none();
    controller.set_input_enabled(game_input);
    camera_activation.write(CameraActivationMessage { active: connected });
    for (mut window, mut cursor) in &mut windows {
        cursor.visible = !game_input;
        cursor.grab_mode = if game_input {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if game_input {
            let center = bevy::math::Vec2::new(window.width() * 0.5, window.height() * 0.5);
            window.set_cursor_position(Some(center));
        }
    }
    applied.0 = Some(state_key);
}

fn spawn_ui_clear_camera(mut commands: Commands) {
    commands.spawn((
        UiClearCamera,
        Camera2d,
        Camera {
            is_active: true,
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..Default::default()
        },
    ));
}

/// The only system that changes Bevy camera activation in response to UI mode.
fn apply_camera_activation(
    mut messages: MessageReader<CameraActivationMessage>,
    controller: Res<ClientPlayerController>,
    mut cameras: Query<(Entity, &mut Camera), (With<Camera3d>, Without<UiClearCamera>)>,
    mut clear_cameras: Query<&mut Camera, With<UiClearCamera>>,
) {
    for message in messages.read() {
        log::debug!("Applying game camera activation: active={}", message.active);
        for (entity, mut camera) in &mut cameras {
            camera.is_active = message.active && controller.is_bound_to(entity);
        }
        for mut clear_camera in &mut clear_cameras {
            // The clear camera renders an otherwise empty 2D pass with a black
            // clear color, replacing every prior world pixel in Web UI mode.
            clear_camera.is_active = !message.active;
        }
    }
}

fn sync_runtime_settings(
    config: Res<roundo_cli::ClientConfigStore>,
    mut input: ResMut<ClientMarionetteInputSettings>,
    mut bindings: ResMut<ClientKeyBindings>,
    mut presence: ResMut<ClientPresenceSettings>,
    mut targeting: ResMut<ClientVoxelRaycastSettings>,
) {
    input.mouse_sensitivity = config.0.settings.controls.mouse_sensitivity;
    input.camera_move_speed = config.0.settings.camera.move_speed;
    *bindings = crate::config::runtime_key_bindings(&config.0.settings);
    presence.set_joinable_world_radius(config.0.settings.world.joinable_world_radius);
    targeting.set_max_distance(config.0.settings.camera.voxel_raycast_distance);
}

fn escape_opens_pause(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<UiLifecycleManager>,
    network: Res<ClientNetworkManager>,
    mut navigation: bevy::ecs::system::NonSendMut<UiNavigationExecutor>,
) {
    if !keys.just_pressed(KeyCode::Escape)
        || !matches!(network.status().status, ClientConnectionStatus::Connected)
        || state.focused_declaration().is_some()
    {
        return;
    }
    let target = match state.resolve_slot_path("roundo.pause-menu", None) {
        Ok(target) => target,
        Err(error) => {
            log::error!("cannot resolve pause UI: {error}");
            return;
        }
    };
    if let Err(error) = navigation.open(&mut state, UiCommandSource::Host, target) {
        log::error!("cannot open pause UI: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::authoritative_lifecycle_state;
    use roundo_cli::client_network::ClientConnectionStatus;
    use roundo_webui::UiLifecycleState;

    #[test]
    fn connecting_stays_under_disconnected_root_until_connected() {
        assert_eq!(
            authoritative_lifecycle_state(
                UiLifecycleState::Disconnected,
                &ClientConnectionStatus::Connecting,
            ),
            UiLifecycleState::Disconnected
        );
        assert_eq!(
            authoritative_lifecycle_state(
                UiLifecycleState::Disconnected,
                &ClientConnectionStatus::Connected,
            ),
            UiLifecycleState::Connected
        );
    }

    #[test]
    fn transient_connected_session_states_preserve_connected_root() {
        for status in [
            ClientConnectionStatus::Reconnecting,
            ClientConnectionStatus::Error {
                message: "temporary".into(),
            },
            ClientConnectionStatus::Connecting,
        ] {
            assert_eq!(
                authoritative_lifecycle_state(UiLifecycleState::Connected, &status),
                UiLifecycleState::Connected
            );
        }
        assert_eq!(
            authoritative_lifecycle_state(
                UiLifecycleState::Connected,
                &ClientConnectionStatus::Disconnected,
            ),
            UiLifecycleState::Disconnected
        );
    }
}
