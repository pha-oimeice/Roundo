mod s0_main_menu;
mod s1_server_selection;
mod s2_connecting;
mod s3_in_game;
mod s4_settings;
mod s5_pause_menu;
mod server_probe;
mod state;
mod widgets;

use crate::{
    config::{self, ServerEntry},
    network::{ActiveClientConnection, ClientConnectionStatus},
};
use bevy::{
    app::AppExit,
    ecs::system::SystemParam,
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::InputFocus,
    prelude::*,
    text::EditableText,
    ui::{InteractionDisabled, RelativeCursorPosition},
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use roundo_character::ClientCharacterIpc;
use roundo_marionette::{
    ClientMarionetteInputSettings, ClientMarionetteIpc, ClientPlayerController,
    ClientPlayerControllerTarget, ControllerCamera,
};
use s0_main_menu::IntroLogo;
use s1_server_selection::{
    GameAddressInput, HttpsAddressInput, ServerListEntry, ServerListScrollArea, ServerNameInput,
};
use server_probe::ServerProbeManager;
use state::{ClientUiEvent, ClientUiPhase, ClientUiState, ServerMutation, ServerSelectionView};
use static_voxel::StaticVoxelClientIpc;
use widgets::{BUTTON_HOVERED, BUTTON_NORMAL, BUTTON_PRESSED, ClientUiRoot, UiButtonAction};

const STARTUP_INTRO_DURATION: f32 = 2.4;

pub struct RoundoClientUiPlugin {
    marionette_ipc: ClientMarionetteIpc,
    character_ipc: ClientCharacterIpc,
    static_voxel_ipc: StaticVoxelClientIpc,
}

impl RoundoClientUiPlugin {
    pub fn new(
        marionette_ipc: ClientMarionetteIpc,
        character_ipc: ClientCharacterIpc,
        static_voxel_ipc: StaticVoxelClientIpc,
    ) -> Self {
        Self {
            marionette_ipc,
            character_ipc,
            static_voxel_ipc,
        }
    }
}

impl Plugin for RoundoClientUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClientNetworkManager {
            marionette_ipc: self.marionette_ipc.clone(),
            character_ipc: self.character_ipc.clone(),
            static_voxel_ipc: self.static_voxel_ipc.clone(),
            active: None,
        })
        .init_resource::<ClientUiState>()
        .init_resource::<ServerProbeManager>()
        .init_resource::<ServerListUiMemory>()
        .init_resource::<StartupIntro>()
        .add_systems(Startup, spawn_ui_camera)
        .add_systems(
            Update,
            (
                animate_startup_intro,
                observe_connection_status,
                poll_server_probes,
                handle_server_list_scroll,
                remember_server_list_scroll,
                handle_escape,
                handle_server_selection,
                handle_ui_buttons,
                refresh_servers_on_s1_entry,
                update_button_colors,
                rebuild_ui,
                sync_gameplay_mode,
            )
                .chain(),
        );
    }
}

#[derive(Resource)]
struct ClientNetworkManager {
    marionette_ipc: ClientMarionetteIpc,
    character_ipc: ClientCharacterIpc,
    static_voxel_ipc: StaticVoxelClientIpc,
    active: Option<ActiveClientConnection>,
}

impl ClientNetworkManager {
    fn connect(&mut self, server: &ServerEntry) -> Result<(), String> {
        self.disconnect();
        self.active = Some(ActiveClientConnection::start(
            server,
            self.marionette_ipc.clone(),
            self.character_ipc.clone(),
            self.static_voxel_ipc.clone(),
        )?);
        Ok(())
    }

    fn disconnect(&mut self) {
        if let Some(active) = self.active.take() {
            active.shutdown();
        }
    }

    fn status(&self) -> Option<ClientConnectionStatus> {
        self.active.as_ref().map(ActiveClientConnection::status)
    }

    fn server_name(&self) -> Option<&str> {
        self.active.as_ref().map(ActiveClientConnection::name)
    }
}

#[derive(Resource)]
struct StartupIntro {
    timer: Timer,
    finished: bool,
}

#[derive(Resource, Default)]
struct ServerListUiMemory {
    scroll_y: f32,
}

impl Default for StartupIntro {
    fn default() -> Self {
        Self {
            timer: Timer::from_seconds(STARTUP_INTRO_DURATION, TimerMode::Once),
            finished: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum UiAction {
    StartGame,
    BackToMainMenu,
    OpenAddServer,
    CancelServerForm,
    SaveServer,
    RefreshServers,
    JoinSelectedServer,
    EditSelectedServer,
    DeleteSelectedServer,
    RetryConnection,
    CancelConnection,
    ContinueGame,
    OpenSettings,
    CloseSettings,
    SensitivityDown,
    SensitivityUp,
    SpeedDown,
    SpeedUp,
    LeaveGameToServers,
    QuitApplication,
}

#[derive(SystemParam)]
struct ServerFormInputs<'w, 's> {
    names: Query<'w, 's, &'static EditableText, With<ServerNameInput>>,
    game_addresses: Query<'w, 's, &'static EditableText, With<GameAddressInput>>,
    https_addresses: Query<'w, 's, &'static EditableText, With<HttpsAddressInput>>,
}

#[derive(SystemParam)]
struct UiViewData<'w> {
    intro: Res<'w, StartupIntro>,
    settings: Res<'w, ClientMarionetteInputSettings>,
    manager: Res<'w, ClientNetworkManager>,
    probes: Res<'w, ServerProbeManager>,
    server_list_memory: Res<'w, ServerListUiMemory>,
}

type EnabledButtonInteractions<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static UiButtonAction),
    (Changed<Interaction>, Without<InteractionDisabled>),
>;

type ButtonColorInteractions<'w, 's> = Query<
    'w,
    's,
    (&'static Interaction, &'static mut BackgroundColor),
    (
        Changed<Interaction>,
        With<Button>,
        Without<InteractionDisabled>,
    ),
>;

fn spawn_ui_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        IsDefaultUiCamera,
        Camera {
            order: 100,
            clear_color: ClearColorConfig::None,
            ..default()
        },
    ));
}

fn animate_startup_intro(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut intro: ResMut<StartupIntro>,
    mut state: ResMut<ClientUiState>,
    mut logos: Query<(&mut TextColor, &mut UiTransform), With<IntroLogo>>,
) {
    if intro.finished || state.phase != ClientUiPhase::S0MainMenu {
        return;
    }

    intro.timer.tick(time.delta());
    let skipped = keyboard.just_pressed(KeyCode::Enter)
        || keyboard.just_pressed(KeyCode::Space)
        || keyboard.just_pressed(KeyCode::Escape)
        || mouse.get_just_pressed().next().is_some();
    let progress = (intro.timer.elapsed_secs() / STARTUP_INTRO_DURATION).clamp(0.0, 1.0);
    let fade_in = (progress / 0.2).clamp(0.0, 1.0);
    let fade_out = ((1.0 - progress) / 0.2).clamp(0.0, 1.0);
    let alpha = fade_in.min(fade_out);
    let eased = 1.0 - (1.0 - progress).powi(3);

    for (mut color, mut transform) in &mut logos {
        color.0 = Color::srgba(0.85, 0.92, 1.0, alpha);
        transform.scale = Vec2::splat(0.88 + 0.12 * eased);
    }

    if skipped || intro.timer.is_finished() {
        intro.finished = true;
        state.touch();
    }
}

fn observe_connection_status(manager: Res<ClientNetworkManager>, mut state: ResMut<ClientUiState>) {
    let status = manager.status();
    if status == state.observed_connection_status {
        return;
    }
    state.observed_connection_status = status.clone();

    match status {
        Some(ClientConnectionStatus::Connecting) => {
            state.set_connecting_message("Connecting...");
        }
        Some(ClientConnectionStatus::Connected) => {
            state.transition(ClientUiEvent::ConnectionSucceeded);
        }
        Some(ClientConnectionStatus::Reconnecting) => {
            state.transition(ClientUiEvent::ConnectionFailed(
                "Connection lost. Reconnecting...".to_string(),
            ));
        }
        Some(ClientConnectionStatus::ConnectionError(error)) => {
            state.transition(ClientUiEvent::ConnectionFailed(format!(
                "Connection failed: {error}\nRetrying automatically..."
            )));
        }
        None if matches!(
            state.phase,
            ClientUiPhase::S3InGame | ClientUiPhase::S4Settings | ClientUiPhase::S5PauseMenu
        ) =>
        {
            state.transition(ClientUiEvent::GameEnded)
        }
        None => {}
    }
}

fn poll_server_probes(mut probes: ResMut<ServerProbeManager>, mut state: ResMut<ClientUiState>) {
    if probes.poll() {
        state.touch();
    }
}

fn handle_server_list_scroll(
    mut mouse_wheel: MessageReader<MouseWheel>,
    mut scroll_areas: Query<
        (&mut ScrollPosition, &ComputedNode, &RelativeCursorPosition),
        With<ServerListScrollArea>,
    >,
) {
    let scroll_delta = mouse_wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y * 30.0,
            MouseScrollUnit::Pixel => event.y,
        })
        .sum::<f32>();
    if scroll_delta == 0.0 {
        return;
    }

    for (mut position, computed, cursor) in &mut scroll_areas {
        if !cursor.cursor_over() {
            continue;
        }
        let range = (computed.content_size().y - computed.size().y).max(0.0)
            * computed.inverse_scale_factor;
        position.y = (position.y - scroll_delta).clamp(0.0, range);
    }
}

fn remember_server_list_scroll(
    scroll_areas: Query<&ScrollPosition, Changed<ScrollPosition>>,
    mut memory: ResMut<ServerListUiMemory>,
) {
    if let Some(position) = scroll_areas.iter().next() {
        memory.scroll_y = position.y;
    }
}

fn handle_escape(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<ClientUiState>,
    mut manager: ResMut<ClientNetworkManager>,
) {
    if !keyboard.just_pressed(KeyCode::Escape) {
        return;
    }

    match state.phase {
        ClientUiPhase::S0MainMenu => {}
        ClientUiPhase::S1ServerSelection => {
            if state.server_selection_view != ServerSelectionView::List {
                state.cancel_server_form();
            } else {
                state.transition(ClientUiEvent::BackToMainMenu);
            }
        }
        ClientUiPhase::S2Connecting => {
            manager.disconnect();
            state.transition(ClientUiEvent::CancelConnection);
        }
        ClientUiPhase::S3InGame => state.transition(ClientUiEvent::PauseGame),
        ClientUiPhase::S4Settings => state.transition(ClientUiEvent::CloseSettings),
        ClientUiPhase::S5PauseMenu => state.transition(ClientUiEvent::ContinueGame),
    }
}

fn handle_server_selection(
    interactions: Query<(&Interaction, &ServerListEntry), Changed<Interaction>>,
    mut state: ResMut<ClientUiState>,
) {
    for (interaction, entry) in &interactions {
        if *interaction == Interaction::Pressed {
            state.select_server(entry.0);
        }
    }
}

fn handle_ui_buttons(
    interactions: EnabledButtonInteractions,
    form_inputs: ServerFormInputs,
    mut state: ResMut<ClientUiState>,
    mut manager: ResMut<ClientNetworkManager>,
    mut settings: ResMut<ClientMarionetteInputSettings>,
    mut probes: ResMut<ServerProbeManager>,
    mut app_exit: MessageWriter<AppExit>,
) {
    for (interaction, action) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }

        match action.0 {
            UiAction::StartGame => {
                state.transition(ClientUiEvent::StartGame);
            }
            UiAction::BackToMainMenu => {
                state.transition(ClientUiEvent::BackToMainMenu);
            }
            UiAction::OpenAddServer => {
                state.begin_add_server();
            }
            UiAction::CancelServerForm => {
                state.cancel_server_form();
            }
            UiAction::SaveServer => {
                let Some(name) = form_inputs.names.iter().next() else {
                    continue;
                };
                let Some(game_addr) = form_inputs.game_addresses.iter().next() else {
                    continue;
                };
                let Some(https_addr) = form_inputs.https_addresses.iter().next() else {
                    continue;
                };
                let server = ServerEntry {
                    name: name.value().to_string(),
                    game_addr: game_addr.value().to_string(),
                    https_addr: https_addr.value().to_string(),
                };
                let Some(mutation) = state.save_server_form(server) else {
                    continue;
                };
                let success_message = match mutation {
                    ServerMutation::Added(_) => "Server added",
                    ServerMutation::Edited(_) => "Server edited",
                };
                match mutation {
                    ServerMutation::Added(index) => {
                        probes.record_added(index);
                    }
                    ServerMutation::Edited(index) => probes.record_edited(index),
                }
                state.status_message = match config::save_servers(&state.servers) {
                    Ok(()) => success_message.to_string(),
                    Err(error) => {
                        format!("{success_message}, but was not saved: {error}")
                    }
                };
            }
            UiAction::RefreshServers => {
                probes.refresh(&state.servers);
                state.status_message.clear();
                state.touch();
            }
            UiAction::JoinSelectedServer => {
                let Some(index) = state.selected_server else {
                    continue;
                };
                let Some(server) = state.servers.get(index).cloned() else {
                    continue;
                };
                attempt_connection(&mut state, &mut manager, server, true);
            }
            UiAction::EditSelectedServer => {
                state.begin_edit_selected_server();
            }
            UiAction::DeleteSelectedServer => {
                let Some((index, _)) = state.delete_selected_server() else {
                    continue;
                };
                probes.record_deleted(index);
                state.status_message = match config::save_servers(&state.servers) {
                    Ok(()) => "Server deleted".to_string(),
                    Err(error) => {
                        format!("Server deleted, but was not saved: {error}")
                    }
                };
            }
            UiAction::RetryConnection => {
                let Some(server) = state.connecting_server.clone() else {
                    continue;
                };
                attempt_connection(&mut state, &mut manager, server, false);
            }
            UiAction::CancelConnection => {
                manager.disconnect();
                state.transition(ClientUiEvent::CancelConnection);
            }
            UiAction::ContinueGame => state.transition(ClientUiEvent::ContinueGame),
            UiAction::OpenSettings => state.transition(ClientUiEvent::OpenSettings),
            UiAction::CloseSettings => state.transition(ClientUiEvent::CloseSettings),
            UiAction::SensitivityDown => {
                settings.mouse_sensitivity =
                    (settings.mouse_sensitivity - 0.0005).clamp(0.0005, 0.01);
                state.touch();
            }
            UiAction::SensitivityUp => {
                settings.mouse_sensitivity =
                    (settings.mouse_sensitivity + 0.0005).clamp(0.0005, 0.01);
                state.touch();
            }
            UiAction::SpeedDown => {
                settings.camera_move_speed = (settings.camera_move_speed - 1.0).clamp(1.0, 50.0);
                state.touch();
            }
            UiAction::SpeedUp => {
                settings.camera_move_speed = (settings.camera_move_speed + 1.0).clamp(1.0, 50.0);
                state.touch();
            }
            UiAction::LeaveGameToServers => {
                manager.disconnect();
                state.transition(ClientUiEvent::LeaveGameToServers);
            }
            UiAction::QuitApplication => {
                manager.disconnect();
                app_exit.write(AppExit::Success);
            }
        }
    }
}

fn refresh_servers_on_s1_entry(
    state: Res<ClientUiState>,
    mut previous_phase: Local<ClientUiPhase>,
    mut probes: ResMut<ServerProbeManager>,
    mut memory: ResMut<ServerListUiMemory>,
) {
    if entered_server_selection(*previous_phase, state.phase) {
        memory.scroll_y = 0.0;
        probes.refresh(&state.servers);
    }
    *previous_phase = state.phase;
}

fn entered_server_selection(previous: ClientUiPhase, current: ClientUiPhase) -> bool {
    current == ClientUiPhase::S1ServerSelection && previous != ClientUiPhase::S1ServerSelection
}

fn attempt_connection(
    state: &mut ClientUiState,
    manager: &mut ClientNetworkManager,
    server: ServerEntry,
    begin: bool,
) {
    if begin {
        state.transition(ClientUiEvent::BeginConnection(server.clone()));
    } else {
        state.observed_connection_status = Some(ClientConnectionStatus::Connecting);
        state.set_connecting_message("Connecting...");
    }

    if let Err(error) = manager.connect(&server) {
        state.observed_connection_status = None;
        state.transition(ClientUiEvent::ConnectionFailed(format!(
            "Connection failed: {error}"
        )));
    }
}

fn update_button_colors(mut buttons: ButtonColorInteractions) {
    for (interaction, mut background) in &mut buttons {
        background.0 = match interaction {
            Interaction::Pressed => BUTTON_PRESSED,
            Interaction::Hovered => BUTTON_HOVERED,
            Interaction::None => BUTTON_NORMAL,
        };
    }
}

fn rebuild_ui(
    mut commands: Commands,
    state: Res<ClientUiState>,
    view: UiViewData,
    roots: Query<Entity, With<ClientUiRoot>>,
    mut input_focus: ResMut<InputFocus>,
) {
    if !state.is_changed() {
        return;
    }

    input_focus.clear();
    for root in &roots {
        commands.entity(root).despawn();
    }
    if state.phase == ClientUiPhase::S3InGame && !s3_in_game::renders_overlay() {
        return;
    }

    let root_color = if state.world_visible() {
        Color::srgba(0.0, 0.0, 0.0, 0.72)
    } else {
        Color::srgb(0.035, 0.045, 0.065)
    };
    let root = commands
        .spawn((
            ClientUiRoot,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(root_color),
        ))
        .id();
    let panel = commands
        .spawn((
            Node {
                width: px(if state.phase == ClientUiPhase::S1ServerSelection {
                    720
                } else {
                    460
                }),
                height: if state.phase == ClientUiPhase::S1ServerSelection {
                    percent(88)
                } else {
                    Val::Auto
                },
                min_height: px(300),
                max_height: percent(92),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                justify_content: if state.phase == ClientUiPhase::S1ServerSelection {
                    JustifyContent::FlexStart
                } else {
                    JustifyContent::Center
                },
                row_gap: px(14),
                padding: px(28).all(),
                border: px(1).all(),
                border_radius: BorderRadius::all(px(12)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.08, 0.095, 0.13)),
            BorderColor::all(Color::srgb(0.25, 0.3, 0.4)),
        ))
        .id();
    commands.entity(root).add_child(panel);

    match state.phase {
        ClientUiPhase::S0MainMenu => {
            s0_main_menu::spawn(&mut commands, panel, view.intro.finished);
        }
        ClientUiPhase::S1ServerSelection => {
            s1_server_selection::spawn(
                &mut commands,
                panel,
                &state,
                &view.probes,
                view.server_list_memory.scroll_y,
            );
        }
        ClientUiPhase::S2Connecting => {
            s2_connecting::spawn(&mut commands, panel, &state);
        }
        ClientUiPhase::S3InGame => {}
        ClientUiPhase::S4Settings => {
            s4_settings::spawn(&mut commands, panel, &view.settings);
        }
        ClientUiPhase::S5PauseMenu => {
            s5_pause_menu::spawn(&mut commands, panel, view.manager.server_name());
        }
    }
}

fn sync_gameplay_mode(
    state: Res<ClientUiState>,
    mut player_controller: ResMut<ClientPlayerController>,
    mut cursor_options: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut cameras: Query<(Entity, &mut Camera, Option<&ControllerCamera>), With<Camera3d>>,
) {
    let playing = state.phase == ClientUiPhase::S3InGame;
    let world_visible = state.world_visible();
    player_controller.set_input_enabled(playing);

    for (entity, mut camera, networked_camera) in &mut cameras {
        let selected = match player_controller.target() {
            ClientPlayerControllerTarget::LocalEntity(target) => entity == target,
            ClientPlayerControllerTarget::NetworkedControllers => networked_camera.is_some(),
        };
        camera.is_active = world_visible && selected;
    }

    if let Some(mut cursor) = cursor_options.iter_mut().next() {
        cursor.visible = !playing;
        cursor.grab_mode = if playing {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientUiEvent, ClientUiPhase, ClientUiState, ServerEntry, entered_server_selection,
        sync_gameplay_mode,
    };
    use bevy::prelude::*;
    use roundo_marionette::ClientPlayerController;

    #[test]
    fn three_dimensional_camera_is_only_active_in_game() {
        let mut app = App::new();
        app.insert_resource(ClientUiState::with_servers(vec![]))
            .init_resource::<ClientPlayerController>()
            .add_systems(Update, sync_gameplay_mode);
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), Camera::default()))
            .id();
        app.world_mut()
            .resource_mut::<ClientPlayerController>()
            .bind_local_entity(camera);

        app.update();
        assert!(!app.world().get::<Camera>(camera).unwrap().is_active);

        let mut state = app.world_mut().resource_mut::<ClientUiState>();
        state.transition(ClientUiEvent::StartGame);
        state.transition(ClientUiEvent::BeginConnection(ServerEntry {
            name: "Test".to_string(),
            game_addr: "127.0.0.1:4000".to_string(),
            https_addr: "127.0.0.1:5000".to_string(),
        }));
        state.transition(ClientUiEvent::ConnectionSucceeded);
        drop(state);

        app.update();
        assert!(app.world().get::<Camera>(camera).unwrap().is_active);
    }

    #[test]
    fn server_probes_only_start_on_s1_entry() {
        assert!(!entered_server_selection(
            ClientUiPhase::S0MainMenu,
            ClientUiPhase::S0MainMenu,
        ));
        assert!(entered_server_selection(
            ClientUiPhase::S0MainMenu,
            ClientUiPhase::S1ServerSelection,
        ));
        assert!(!entered_server_selection(
            ClientUiPhase::S1ServerSelection,
            ClientUiPhase::S1ServerSelection,
        ));
        assert!(!entered_server_selection(
            ClientUiPhase::S1ServerSelection,
            ClientUiPhase::S0MainMenu,
        ));
    }
}
