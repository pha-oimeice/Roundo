//! Client command catalog, adapters, main-world dispatch, and subscribed Client Data.

mod application;
mod catalog;
mod client_data;
mod terminal;
use catalog::*;
use client_data::*;
use terminal::*;

use crate::server_list::{ClientConfigStore, ServerListOutput};

use bevy::{
    app::AppExit,
    prelude::{
        App, IntoScheduleConfigs, MessageWriter, Query, Res, ResMut, Resource, Time, Transform,
        Update,
    },
};
use roundo_cli::{
    CommandDefinition, TerminalInput, UnixCommand, UnixCommandParseError, UnixCommandRegistry,
};
use roundo_marionette::{ClientPlayerController, InputFunctionRegistry};
use roundo_toolbox::request_response_pipe::{
    CommandTransport, ContextualJsonRequestResponseIo, JsonSubmitError, RequestCall,
    RequestResponsePipe, ResponsePoll,
};
use roundo_user_config::{
    ClientInputBindingConfig, ClientInputKey, ClientInputSlotConfig, ServerEntry,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::VecDeque, process::Command};

const MAX_JSON_COMMANDS_PER_UPDATE: usize = 64;
const SERVER_LIST_DATA: &str = "client.servers";
const SERVER_STATUS_DATA: &str = "client.connection";
const CONTROL_STATUS_DATA: &str = "client.control";
const SETTINGS_DATA: &str = "client.settings";
const BINDINGS_DATA: &str = "client.bindings";
const HUD_DATA: &str = "client.hud";
const HUD_SYNC_INTERVAL_SECS: f32 = 0.05;

/// ECS-owned consumer side of the bounded client command queue.
#[derive(Resource)]
pub(crate) struct ClientCommandPipe(
    RequestResponsePipe<CommandTransport<roundo_webui::UiCommandSource>, serde_json::Value>,
);

impl ClientCommandPipe {
    pub(crate) fn bounded(capacity: usize) -> Self {
        Self(RequestResponsePipe::bounded(capacity))
    }

    pub(crate) fn io(
        &self,
    ) -> ContextualJsonRequestResponseIo<roundo_webui::UiCommandSource, serde_json::Value> {
        ContextualJsonRequestResponseIo::new(self.0.io())
    }

    fn try_receive(
        &self,
    ) -> Option<(
        CommandTransport<roundo_webui::UiCommandSource>,
        roundo_toolbox::request_response_pipe::ResponseSender<serde_json::Value>,
    )> {
        self.0.try_receive()
    }
}

/// Main-world phase that executes commands before the Web UI platform phase.
#[derive(Clone, Debug, Eq, Hash, PartialEq, bevy::prelude::SystemSet)]
pub(crate) enum ClientCommandSystemSet {
    ExecuteAndPublish,
}

/// Installs the client-owned command catalog and its terminal/Web UI adapters.
pub(crate) struct ClientCommandsPlugin;

impl bevy::prelude::Plugin for ClientCommandsPlugin {
    fn build(&self, app: &mut App) {
        configure(app);
    }
}

/// Read-only diagnostic cache consumed by the HUD command. Producers update
/// this resource; Web UI only crosses the small `hud.show` command seam.
#[derive(Resource, Clone, Debug, Serialize)]
pub struct HudCache {
    /// Statistics over at most the latest 120 valid frame durations.
    pub fps: HudFps,
    /// Current bound-camera position, absent when no valid camera is available.
    pub position: Option<HudPosition>,
    /// Caller-defined owned targeting snapshot.
    pub target: Option<serde_json::Value>,
    #[serde(skip)]
    frames: VecDeque<f32>,
}

/// Frames-per-second statistics derived from finite positive frame durations.
#[derive(Clone, Debug, Serialize)]
pub struct HudFps {
    pub current: f32,
    pub average: f32,
    pub min: f32,
    pub max: f32,
}
/// Camera position represented as raw world coordinates and floored integer coordinates.
#[derive(Clone, Debug, Serialize)]
pub struct HudPosition {
    pub absolute: [f32; 3],
    /// Each world component is floored and converted to `i32`; this is not a Chunk offset.
    pub chunk_relative: [i32; 3],
}

impl Default for HudCache {
    fn default() -> Self {
        Self {
            fps: HudFps {
                current: 0.0,
                average: 0.0,
                min: 0.0,
                max: 0.0,
            },
            position: None,
            target: None,
            frames: VecDeque::new(),
        }
    }
}

impl HudCache {
    /// Replaces the owned targeting snapshot exposed by `hud.show` and Client Data.
    pub fn set_target(&mut self, target: Option<serde_json::Value>) {
        self.target = target;
    }
    fn record_frame(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        self.frames.push_back(delta_seconds);
        if self.frames.len() > 120 {
            self.frames.pop_front();
        }
        let fps: Vec<f32> = self.frames.iter().map(|delta| delta.recip()).collect();
        self.fps.current = delta_seconds.recip();
        self.fps.average = fps.iter().sum::<f32>() / fps.len() as f32;
        self.fps.min = fps.iter().copied().fold(f32::INFINITY, f32::min);
        self.fps.max = fps.iter().copied().fold(0.0, f32::max);
    }
}

#[derive(bevy::prelude::Resource, Default)]
struct DevLevel(u8);

/// Accepts an absolute lowercase `http`/`https` URL without credentials or whitespace.
///
/// This is a narrow launch-safety check, not full URL parsing or host validation.
fn validate_external_url(value: &str) -> Result<&str, roundo_cli::json_command::CommandError> {
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(roundo_cli::json_command::CommandError::new(
            "invalid_external_url",
            "external URL contains whitespace or control characters",
        ));
    }
    let Some((scheme, remainder)) = value.split_once("://") else {
        return Err(roundo_cli::json_command::CommandError::new(
            "invalid_external_url",
            "external URL must be an absolute http or https URL",
        ));
    };
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    if !matches!(scheme, "http" | "https")
        || authority.is_empty()
        || authority.contains('@')
        || authority.chars().any(char::is_whitespace)
    {
        return Err(roundo_cli::json_command::CommandError::new(
            "invalid_external_url",
            "external URL must use http or https and contain a host without credentials",
        ));
    }
    Ok(value)
}

/// Spawns the platform URL opener without waiting for it to finish.
///
/// Success means only that the helper process started.
fn open_external_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command.spawn().map(|_| ())
}

fn configure_webui_ipc_order(app: &mut App) {
    app.configure_sets(
        Update,
        ClientCommandSystemSet::ExecuteAndPublish.before(roundo_webui::WebUiSystemSet::Platform),
    );
}

/// Installs missing client resources and the ordered command/data systems.
///
/// Preinserted command pipes and configuration stores are preserved.
fn configure(app: &mut App) {
    configure_webui_ipc_order(app);
    if !app.world().contains_resource::<ClientCommandPipe>() {
        app.insert_resource(ClientCommandPipe::bounded(256));
    }
    if !app.world().contains_resource::<ClientConfigStore>() {
        app.insert_resource(ClientConfigStore::new(crate::config::load_config()));
    }
    crate::config::validate_input_configuration(
        app.world().resource::<ClientConfigStore>().settings(),
        app.world().resource::<InputFunctionRegistry>(),
    )
    .unwrap_or_else(|error| panic!("invalid client shortcut configuration: {error}"));
    app.init_resource::<DevLevel>();
    app.init_resource::<HudCache>();
    app.init_resource::<ClientDataSyncState>();
    app.init_resource::<ClientUnixAdapter>();
    app.init_resource::<TerminalResponses>();
    app.add_systems(
        Update,
        (
            process_commands,
            process_json_commands,
            update_hud_cache,
            sync_subscribed_client_data,
        )
            .chain()
            .in_set(ClientCommandSystemSet::ExecuteAndPublish),
    );
}

/// Drains source-neutral JSON requests on the Bevy main world. This is the
/// execution side of the Web UI/terminal command seam.
fn process_json_commands(
    pipe: Res<ClientCommandPipe>,
    mut config: ResMut<ClientConfigStore>,
    mut network: ResMut<crate::network::ClientNetworkManager>,
    mut dev_level: ResMut<DevLevel>,
    controller: Res<ClientPlayerController>,
    cameras: Query<&Transform>,
    mut app_exit: MessageWriter<AppExit>,
    mut webui: Option<ResMut<roundo_webui::UiLifecycleManager>>,
    mut navigation: Option<bevy::ecs::system::NonSendMut<roundo_webui::UiNavigationExecutor>>,
    mut scene: Option<ResMut<crate::scene::ClientSceneManager>>,
    hud: Res<HudCache>,
    input_registry: Res<InputFunctionRegistry>,
) {
    for _ in 0..MAX_JSON_COMMANDS_PER_UPDATE {
        let Some((transport, reply)) = pipe.try_receive() else {
            break;
        };
        let request = transport.command;
        let ui_source = command_transport_source(transport.context);
        let quit_accepted = application::ClientCommandApplication {
            config: &mut config,
            network: &mut network,
            dev_level: &mut dev_level,
            camera_position: controller
                .camera()
                .and_then(|entity| {
                    let camera = cameras.get(entity);
                    camera.ok()
                })
                .map(|transform| transform.translation.to_array()),
            webui: webui.as_deref_mut(),
            navigation: navigation.as_deref_mut(),
            scene: scene.as_deref_mut(),
            hud: &hud,
            input_registry: &input_registry,
        }
        .execute(request, ui_source, reply);
        if quit_accepted {
            let _exit_message = app_exit.write(AppExit::Success);
        }
    }
}

/// Converts adapter-owned transport metadata into host authority. This is the
/// only place where a WebView identity becomes a UI command source; the typed
/// JSON command remains unchanged and source-neutral.
fn command_transport_source(
    source: roundo_webui::UiCommandSource,
) -> Option<roundo_webui::UiInstanceId> {
    match source {
        roundo_webui::UiCommandSource::Host => None,
        roundo_webui::UiCommandSource::WebView(instance) => Some(instance),
    }
}

fn stale_ui_source_response(request: &serde_json::Value) -> serde_json::Value {
    let command_value = &request["command"];
    let command = command_value.as_str().unwrap_or_default().to_owned();
    serde_json::to_value(roundo_cli::json_command::CommandResult {
        version: roundo_cli::json_command::COMMAND_VERSION,
        command,
        ok: false,
        data: None,
        error: Some(roundo_cli::json_command::CommandError::new(
            "stale_ui_instance",
            "WebView command source is no longer live and loaded",
        )),
    })
    .expect("serializable stale UI command result")
}

/// Maps lifecycle-domain failures onto command error categories.
fn map_ui_lifecycle_error(
    error: roundo_webui::UiLifecycleError,
) -> roundo_cli::json_command::CommandError {
    let code = match error {
        roundo_webui::UiLifecycleError::UiInstanceLimit => "ui_instance_limit",
        roundo_webui::UiLifecycleError::DuplicatePendingOpen => "ui_open_pending",
        roundo_webui::UiLifecycleError::UnknownImport(_) => "unknown_ui_import",
        roundo_webui::UiLifecycleError::StaleUiInstance => "stale_ui_instance",
        roundo_webui::UiLifecycleError::CannotCloseUiRoot => "cannot_close_ui_root",
        roundo_webui::UiLifecycleError::Registry(_) => "invalid_ui_resource",
        roundo_webui::UiLifecycleError::NavigationFailed(_) => "ui_navigation_failed",
        roundo_webui::UiLifecycleError::LoadTimeout => "ui_load_timeout",
        roundo_webui::UiLifecycleError::InternalTree(_) => "internal_ui_error",
    };
    roundo_cli::json_command::CommandError::new(code, error.to_string())
}

// Samples only the currently bound valid camera; missing cameras clear position.
