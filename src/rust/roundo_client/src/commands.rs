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
        App, IntoScheduleConfigs, Mut, Query, Res, ResMut, Resource, Time, Transform, Update, World,
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
use std::{cell::RefCell, collections::VecDeque};

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
    RequestResponsePipe<CommandTransport<roundo_ui::UiCommandSource>, serde_json::Value>,
);

impl ClientCommandPipe {
    pub(crate) fn bounded(capacity: usize) -> Self {
        Self(RequestResponsePipe::bounded(capacity))
    }

    pub(crate) fn io(
        &self,
    ) -> ContextualJsonRequestResponseIo<roundo_ui::UiCommandSource, serde_json::Value> {
        ContextualJsonRequestResponseIo::new(self.0.io())
    }

    fn try_receive(
        &self,
    ) -> Option<(
        CommandTransport<roundo_ui::UiCommandSource>,
        roundo_toolbox::request_response_pipe::ResponseSender<serde_json::Value>,
    )> {
        self.0.try_receive()
    }
}

/// Main-world command and snapshot phase, before native widget projection.
#[derive(Clone, Debug, Eq, Hash, PartialEq, bevy::prelude::SystemSet)]
pub(crate) enum ClientCommandSystemSet {
    ExecuteAndPublish,
}

/// Installs the client-owned command catalog and terminal/native UI adapters.
pub(crate) struct ClientCommandsPlugin;

impl bevy::prelude::Plugin for ClientCommandsPlugin {
    fn build(&self, app: &mut App) {
        configure(app);
    }
}

/// Read-only diagnostic cache consumed by the HUD command. Producers update
/// this resource; native UI reads its published snapshot.
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
    // Preserve the existing no-wait contract rather than adopting the dependency's
    // timed early-exit probe. The dependency still owns detachment and child reaping;
    // a successful return is not a receipt from the browser that will open the URL.
    open::that_detached_with_timeout(url, std::time::Duration::ZERO)
}

fn configure_ui_order(app: &mut App) {
    app.configure_sets(
        Update,
        ClientCommandSystemSet::ExecuteAndPublish
            .after(roundo_ui::UiSystemSet::Ingress)
            .before(roundo_ui::UiSystemSet::Project),
    );
}

/// Installs missing client resources and the ordered command/data systems.
///
/// Preinserted command pipes and configuration stores are preserved.
fn configure(app: &mut App) {
    configure_ui_order(app);
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
/// execution side of the native UI/terminal command seam.
fn process_json_commands(world: &mut World) {
    for _ in 0..MAX_JSON_COMMANDS_PER_UPDATE {
        let Some((transport, reply)) = world.resource::<ClientCommandPipe>().try_receive() else {
            break;
        };
        if world.contains_resource::<bevy::prelude::State<roundo_scene::Scene>>()
            && roundo_ui::UiLifecycleManager::is_installed(world)
        {
            if let Err(error) = crate::scene::reconcile_scene(world) {
                let command = transport.command["command"].as_str().unwrap_or_default();
                let _ = reply.respond(terminal_error(command, "ui_presentation_failed", error));
                world.write_message(AppExit::error());
                break;
            }
        }
        let camera_position = world
            .resource::<ClientPlayerController>()
            .camera()
            .and_then(|entity| world.get::<Transform>(entity))
            .map(|transform| transform.translation.to_array());
        let quit = {
            world.resource_scope(|world, mut config: Mut<ClientConfigStore>| {
                world.resource_scope(
                    |world, mut network: Mut<crate::network::ClientNetworkManager>| {
                        world.resource_scope(|world, mut dev_level: Mut<DevLevel>| {
                            world.resource_scope(|world, hud: Mut<HudCache>| {
                                world.resource_scope(
                                    |world, input_registry: Mut<InputFunctionRegistry>| {
                                        let installed =
                                            roundo_ui::UiLifecycleManager::is_installed(world);
                                        let mut ui = roundo_ui::UiLifecycleManager { world };
                                        application::ClientCommandApplication {
                                            config: &mut config,
                                            network: &mut network,
                                            dev_level: &mut dev_level,
                                            camera_position,
                                            ui: installed.then_some(&mut ui),
                                            hud: &hud,
                                            input_registry: &input_registry,
                                        }
                                        .execute(
                                            transport.command,
                                            command_transport_source(transport.context),
                                            reply,
                                        )
                                    },
                                )
                            })
                        })
                    },
                )
            })
        };
        crate::scene::apply_pending_transition(world);
        if quit {
            world.write_message(AppExit::Success);
        }
    }
}

/// Converts adapter-owned transport metadata into host authority. This is the
/// only place where a native view identity becomes a UI command source; the typed
/// JSON command remains unchanged and source-neutral.
fn command_transport_source(source: roundo_ui::UiCommandSource) -> Option<roundo_ui::UiInstanceId> {
    match source {
        roundo_ui::UiCommandSource::Host => None,
        roundo_ui::UiCommandSource::Native(instance) => Some(instance),
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
            "native UI command source is no longer live",
        )),
    })
    .expect("serializable stale UI command result")
}

/// Maps lifecycle-domain failures onto command error categories.
fn map_ui_lifecycle_error(
    error: roundo_ui::UiLifecycleError,
) -> roundo_cli::json_command::CommandError {
    let code = match error {
        roundo_ui::UiLifecycleError::UiInstanceLimit => "ui_instance_limit",
        roundo_ui::UiLifecycleError::UnknownImport(_) => "unknown_ui_import",
        roundo_ui::UiLifecycleError::StaleUiInstance => "stale_ui_instance",
        roundo_ui::UiLifecycleError::CannotCloseUiRoot => "cannot_close_ui_root",
        roundo_ui::UiLifecycleError::Registry(_) => "invalid_ui_resource",
        roundo_ui::UiLifecycleError::Presentation(_) => "ui_presentation_failed",
    };
    roundo_cli::json_command::CommandError::new(code, error.to_string())
}

/// Validate resource references against the actual client-owned producers/catalog.
pub(crate) fn validate_ui_registry(
    registry: &roundo_mod_manager::ui::UiCatalogue,
    assets: &roundo_mod_manager::AssetAccess,
    inputs: &InputFunctionRegistry,
) -> Result<(), String> {
    use roundo_mod_manager::ui::schema::{Action, NodeDef};
    assets.with(|manager| registry.validate_manager(manager))?;
    fn actions(actions: &[Action], session: bool) -> Result<(), String> {
        for action in actions {
            match action {
                Action::Command { name, .. }
                    if CLIENT_COMMAND_CATALOG.dev_level(name).is_none() =>
                {
                    return Err(format!("unknown native UI command `{name}`"));
                }
                Action::Debug { .. } if !session => {
                    return Err("debug action requires session_source".into());
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn nodes(definitions: &[NodeDef], session: bool) -> Result<(), String> {
        for node in definitions {
            actions(&node.on_activate, session)?;
            actions(&node.on_change, session)?;
            nodes(&node.children, session)?;
        }
        Ok(())
    }
    for name in registry.names() {
        let key = registry.key(name).expect("UI catalogue identity");
        assets.with(|assets| -> Result<(), String> {
            let handle = assets.load(key).map_err(|error| error.to_string())?;
            assets
                .read(&handle, |prototype| -> Result<(), String> {
                    let page = match prototype {
                        roundo_mod_manager::AssetPrototype::Ui(ui) => &ui.page,
                        _ => return Err(format!("{name} is not a UI prototype")),
                    };
                    for source in page.sources.values() {
                        if ![
                            SERVER_LIST_DATA,
                            SERVER_STATUS_DATA,
                            CONTROL_STATUS_DATA,
                            SETTINGS_DATA,
                            BINDINGS_DATA,
                            HUD_DATA,
                            "client.inventory",
                            "client.debug",
                        ]
                        .contains(&source.as_str())
                        {
                            return Err(format!("{}: unknown Client Data source `{source}`", name));
                        }
                    }
                    let session = page.session_source.is_some();
                    actions(&page.on_open, session)?;
                    for watcher in page.on_data.values() {
                        actions(watcher, session)?;
                    }
                    for (function, shortcut) in &page.shortcuts {
                        if inputs.function(function).is_none() {
                            return Err(format!(
                                "{}: shortcut references unknown input function `{function}`",
                                name
                            ));
                        }
                        actions(shortcut, session)?;
                    }
                    nodes(&page.nodes, session)?;
                    Ok(())
                })
                .map_err(|error| error.to_string())?
        })?;
    }
    Ok(())
}
