//! Client command catalog, adapters, main-world dispatch, and subscribed Client Data.

mod application;

use crate::{
    ClientCommandDefinition, ClientCommandPipe, MAX_JSON_COMMANDS_PER_UPDATE, TerminalInput,
    UnixCommand, UnixCommandParseError, UnixCommandRegistry,
};
use bevy::{
    app::AppExit,
    prelude::{
        App, IntoScheduleConfigs, MessageWriter, Query, Res, ResMut, Resource, Time, Transform,
        Update,
    },
};
use roundo_marionette::{ClientPlayerController, InputRegistry};
use roundo_toolbox::request_response_pipe::{JsonSubmitError, RequestCall};
use roundo_user_config::{ClientConfig, ClientInputBindingConfig, ClientInputKey, ServerEntry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    process::Command,
};

const CLIENT_CONFIG_FILE: &str = "roundo-client-config.toml";
const SERVER_LIST_DATA: &str = "client.servers";
const SERVER_STATUS_DATA: &str = "client.connection";
const SETTINGS_DATA: &str = "client.settings";
const BINDINGS_DATA: &str = "client.bindings";
const HUD_DATA: &str = "client.hud";
const HUD_SYNC_INTERVAL_SECS: f32 = 0.05;

/// Last published Client Data snapshots and source-specific change counters.
#[derive(Resource, Default)]
struct ClientDataSyncState {
    subscription_generation: u64,
    server_list_revision: u64,
    last_snapshots: BTreeMap<&'static str, serde_json::Value>,
    hud_elapsed_secs: f32,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ServerIndexArguments {
    index: usize,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UiOpenArguments {
    import: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UiBackArguments {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenExternalUrlArguments {
    url: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommandHelpArguments {
    command: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommandSchemaArguments {
    command: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DevArguments {
    level: Option<u8>,
}
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ServerEntryArguments {
    name: String,
    address: String,
}
impl From<ServerEntryArguments> for ServerEntry {
    fn from(value: ServerEntryArguments) -> Self {
        Self {
            name: value.name,
            address: value.address,
        }
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ServerEditArguments {
    index: usize,
    server: ServerEntryArguments,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SettingsSetArguments {
    key: String,
    value: f32,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BindingArguments {
    slot: String,
    key: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BindingsReplaceArguments {
    old_binding: BindingArguments,
    binding: BindingArguments,
}
impl TryFrom<BindingArguments> for ClientInputBindingConfig {
    type Error = crate::json_command::CommandError;
    fn try_from(value: BindingArguments) -> Result<Self, Self::Error> {
        if value.slot.trim().is_empty() {
            return Err(crate::json_command::CommandError::new(
                "invalid_arguments",
                "binding slot cannot be empty",
            ));
        }
        let key =
            serde_json::from_value(serde_json::Value::String(value.key)).map_err(|error| {
                crate::json_command::CommandError::new("invalid_arguments", error.to_string())
            })?;
        Ok(Self::new(value.slot, key))
    }
}

#[derive(Serialize, JsonSchema)]
struct BindingDisplay {
    slot: String,
    key: String,
}
#[derive(Serialize, JsonSchema)]
struct SupportedKeyDisplay {
    key: String,
    display_name: String,
}
#[derive(Serialize, JsonSchema)]
struct SupportedSlotDisplay {
    slot: String,
    display_name: String,
}
#[derive(Serialize, JsonSchema)]
struct BindingsListOutput {
    bindings: Vec<BindingDisplay>,
    supported_keys: Vec<SupportedKeyDisplay>,
    supported_slots: Vec<SupportedSlotDisplay>,
}
#[derive(Serialize, JsonSchema)]
struct BindingMutationOutput {}

fn key_display(key: ClientInputKey) -> SupportedKeyDisplay {
    use ClientInputKey::*;
    let (key, display_name) = match key {
        Digit0 => ("digit0", "0"),
        Digit1 => ("digit1", "1"),
        Digit2 => ("digit2", "2"),
        Digit3 => ("digit3", "3"),
        Digit4 => ("digit4", "4"),
        Digit5 => ("digit5", "5"),
        Digit6 => ("digit6", "6"),
        Digit7 => ("digit7", "7"),
        Digit8 => ("digit8", "8"),
        Digit9 => ("digit9", "9"),
        Backspace => ("backspace", "Backspace"),
        Tab => ("tab", "Tab"),
        KeyQ => ("key_q", "Q"),
        KeyW => ("key_w", "W"),
        KeyE => ("key_e", "E"),
        KeyR => ("key_r", "R"),
        KeyT => ("key_t", "T"),
        KeyY => ("key_y", "Y"),
        KeyU => ("key_u", "U"),
        KeyI => ("key_i", "I"),
        KeyO => ("key_o", "O"),
        KeyP => ("key_p", "P"),
        CapsLock => ("caps_lock", "Caps Lock"),
        KeyA => ("key_a", "A"),
        KeyS => ("key_s", "S"),
        KeyD => ("key_d", "D"),
        KeyF => ("key_f", "F"),
        KeyG => ("key_g", "G"),
        KeyH => ("key_h", "H"),
        KeyJ => ("key_j", "J"),
        KeyK => ("key_k", "K"),
        KeyL => ("key_l", "L"),
        Enter => ("enter", "Enter"),
        ShiftLeft => ("shift_left", "Left Shift"),
        KeyZ => ("key_z", "Z"),
        KeyX => ("key_x", "X"),
        KeyC => ("key_c", "C"),
        KeyV => ("key_v", "V"),
        KeyB => ("key_b", "B"),
        KeyN => ("key_n", "N"),
        KeyM => ("key_m", "M"),
        ShiftRight => ("shift_right", "Right Shift"),
        ControlLeft => ("control_left", "Left Control"),
        AltLeft => ("alt_left", "Left Alt"),
        Space => ("space", "Space"),
        AltRight => ("alt_right", "Right Alt"),
        ControlRight => ("control_right", "Right Control"),
        ArrowLeft => ("arrow_left", "Left Arrow"),
        ArrowUp => ("arrow_up", "Up Arrow"),
        ArrowDown => ("arrow_down", "Down Arrow"),
        ArrowRight => ("arrow_right", "Right Arrow"),
        F1 => ("f1", "F1"),
        MouseLeft => ("mouse_left", "Left Mouse Button"),
        MouseRight => ("mouse_right", "Right Mouse Button"),
        MouseMiddle => ("mouse_middle", "Middle Mouse Button"),
    };
    SupportedKeyDisplay {
        key: key.into(),
        display_name: display_name.into(),
    }
}

fn supported_keys() -> Vec<SupportedKeyDisplay> {
    use ClientInputKey::*;
    [
        Digit0,
        Digit1,
        Digit2,
        Digit3,
        Digit4,
        Digit5,
        Digit6,
        Digit7,
        Digit8,
        Digit9,
        Backspace,
        Tab,
        KeyQ,
        KeyW,
        KeyE,
        KeyR,
        KeyT,
        KeyY,
        KeyU,
        KeyI,
        KeyO,
        KeyP,
        CapsLock,
        KeyA,
        KeyS,
        KeyD,
        KeyF,
        KeyG,
        KeyH,
        KeyJ,
        KeyK,
        KeyL,
        Enter,
        ShiftLeft,
        KeyZ,
        KeyX,
        KeyC,
        KeyV,
        KeyB,
        KeyN,
        KeyM,
        ShiftRight,
        ControlLeft,
        AltLeft,
        Space,
        AltRight,
        ControlRight,
        ArrowLeft,
        ArrowUp,
        ArrowDown,
        ArrowRight,
        F1,
        MouseLeft,
        MouseRight,
        MouseMiddle,
    ]
    .into_iter()
    .map(key_display)
    .collect()
}

fn binding_display(binding: &ClientInputBindingConfig) -> BindingDisplay {
    BindingDisplay {
        slot: binding.slot.clone(),
        key: key_display(binding.key).key,
    }
}

fn replace_binding(
    bindings: &mut Vec<ClientInputBindingConfig>,
    old_binding: ClientInputBindingConfig,
    replacement: ClientInputBindingConfig,
) -> Result<(), crate::json_command::CommandError> {
    let Some(index) = bindings
        .iter()
        .position(|existing| *existing == old_binding)
    else {
        return Err(crate::json_command::CommandError::new(
            "binding_not_found",
            "binding does not exist",
        ));
    };
    if bindings
        .iter()
        .enumerate()
        .any(|(other, existing)| other != index && *existing == replacement)
    {
        return Err(crate::json_command::CommandError::new(
            "binding_exists",
            "replacement binding already exists",
        ));
    }
    bindings[index] = replacement;
    Ok(())
}

#[derive(Serialize, JsonSchema)]
struct EmptyOutput {}
#[derive(Serialize, JsonSchema)]
struct AcceptedOutput {
    status: String,
}
#[derive(Serialize, JsonSchema)]
struct ServerEntryDisplay {
    name: String,
    address: String,
}
impl From<&ServerEntry> for ServerEntryDisplay {
    fn from(server: &ServerEntry) -> Self {
        Self {
            name: server.name.clone(),
            address: server.address.clone(),
        }
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ConnectionStatusOutput {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error { message: String },
}
#[derive(Serialize, JsonSchema)]
struct ServerStatusOutput {
    status: ConnectionStatusOutput,
    server: Option<ServerEntryDisplay>,
}
#[derive(Serialize, JsonSchema)]
struct ProbeOutput {
    status: String,
    message: Option<String>,
}
#[derive(Serialize, JsonSchema)]
struct ServerListEntryOutput {
    index: usize,
    name: String,
    address: String,
    probe: ProbeOutput,
}
#[derive(Serialize, JsonSchema)]
struct ServerListOutput {
    servers: Vec<ServerListEntryOutput>,
}
#[derive(Serialize, JsonSchema)]
struct ServerRefreshOutput {
    accepted: bool,
    revision: u64,
}
#[derive(Serialize, JsonSchema)]
struct ServerIndexOutput {
    index: usize,
}
#[derive(Serialize, JsonSchema)]
struct SettingDisplay {
    key: String,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    default: f32,
}
#[derive(Serialize, JsonSchema)]
struct SettingsShowOutput {
    settings: Vec<SettingDisplay>,
}
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
enum CommandHelpOutput {
    Commands { commands: Vec<&'static str> },
    Command { command: String, dev_level: u8 },
}
/// A requested schema varies with the command, so this wrapper remains transparent on the stable wire format.
#[derive(Serialize, JsonSchema)]
#[serde(transparent)]
struct CommandSchemaOutput(serde_json::Map<String, serde_json::Value>);
#[derive(Serialize, JsonSchema)]
struct DevOutput {
    level: u8,
}
#[derive(Serialize, JsonSchema)]
struct PositionOutput {
    x: f32,
    y: f32,
    z: f32,
}
#[derive(Deserialize, Serialize, JsonSchema)]
struct HudTargetOutput {
    voxel: String,
    voxel_position: [i32; 3],
    relative_position: [f32; 3],
    chunk: [i32; 3],
    local_coordinate: String,
}
#[derive(Serialize, JsonSchema)]
struct HudFpsOutput {
    current: f32,
    average: f32,
    min: f32,
    max: f32,
}
#[derive(Serialize, JsonSchema)]
struct HudPositionOutput {
    absolute: [f32; 3],
    chunk_relative: [i32; 3],
}
#[derive(Serialize, JsonSchema)]
struct HudShowOutput {
    fps: HudFpsOutput,
    position: Option<HudPositionOutput>,
    target: Option<HudTargetOutput>,
}
#[derive(Serialize, JsonSchema)]
struct UiOpenOutput {
    resource: String,
}

fn server_status_output(
    snapshot: crate::client_network::ClientConnectionSnapshot,
) -> ServerStatusOutput {
    use crate::client_network::ClientConnectionStatus;
    let status = match snapshot.status {
        ClientConnectionStatus::Disconnected => ConnectionStatusOutput::Disconnected,
        ClientConnectionStatus::Connecting => ConnectionStatusOutput::Connecting,
        ClientConnectionStatus::Connected => ConnectionStatusOutput::Connected,
        ClientConnectionStatus::Reconnecting => ConnectionStatusOutput::Reconnecting,
        ClientConnectionStatus::Error { message } => ConnectionStatusOutput::Error { message },
    };
    ServerStatusOutput {
        status,
        server: snapshot.server.as_ref().map(ServerEntryDisplay::from),
    }
}

fn probe_output(probe: crate::client_network::ProbeResult) -> ProbeOutput {
    ProbeOutput {
        status: probe.status.into(),
        message: probe.message,
    }
}

fn server_list_output(
    config: &ClientConfigStore,
    probes: &crate::client_network::ServerProbeManager,
) -> ServerListOutput {
    let servers = config
        .0
        .servers
        .iter()
        .enumerate()
        .map(|(index, server)| ServerListEntryOutput {
            index,
            name: server.name.clone(),
            address: server.address.clone(),
            probe: probe_output(probes.result(index, server)),
        })
        .collect();
    ServerListOutput { servers }
}

fn settings_show_output(config: &ClientConfigStore) -> SettingsShowOutput {
    let settings = &config.0.settings;
    SettingsShowOutput {
        settings: vec![
            SettingDisplay {
                key: "controls.mouse_sensitivity".into(),
                value: settings.controls.mouse_sensitivity,
                min: roundo_user_config::MIN_MOUSE_SENSITIVITY,
                max: roundo_user_config::MAX_MOUSE_SENSITIVITY,
                step: 0.0005,
                default: roundo_user_config::DEFAULT_MOUSE_SENSITIVITY,
            },
            SettingDisplay {
                key: "camera.move_speed".into(),
                value: settings.camera.move_speed,
                min: roundo_user_config::MIN_CAMERA_MOVE_SPEED,
                max: roundo_user_config::MAX_CAMERA_MOVE_SPEED,
                step: 1.0,
                default: roundo_user_config::DEFAULT_CAMERA_MOVE_SPEED,
            },
            SettingDisplay {
                key: "camera.voxel_raycast_distance".into(),
                value: settings.camera.voxel_raycast_distance,
                min: roundo_user_config::MIN_VOXEL_RAYCAST_DISTANCE,
                max: roundo_user_config::MAX_VOXEL_RAYCAST_DISTANCE,
                step: 1.0,
                default: roundo_user_config::DEFAULT_VOXEL_RAYCAST_DISTANCE,
            },
            SettingDisplay {
                key: "world.joinable_world_radius".into(),
                value: settings.world.joinable_world_radius,
                min: roundo_user_config::MIN_JOINABLE_WORLD_RADIUS,
                max: roundo_user_config::MAX_JOINABLE_WORLD_RADIUS,
                step: 0.5,
                default: roundo_user_config::DEFAULT_JOINABLE_WORLD_RADIUS,
            },
            SettingDisplay {
                key: "world.chunk_view_distance".into(),
                value: settings.world.chunk_view_distance,
                min: roundo_user_config::MIN_CHUNK_VIEW_DISTANCE,
                max: roundo_user_config::MAX_CHUNK_VIEW_DISTANCE,
                step: 1.0,
                default: roundo_user_config::DEFAULT_CHUNK_VIEW_DISTANCE,
            },
        ],
    }
}

fn validate_binding_slot(
    binding: &ClientInputBindingConfig,
    registry: Option<&InputRegistry>,
) -> Result<(), crate::json_command::CommandError> {
    if registry.is_some_and(|registry| !registry.contains_slot(&binding.slot)) {
        return Err(crate::json_command::CommandError::new(
            "unknown_input_slot",
            format!("input slot `{}` is not registered", binding.slot),
        ));
    }
    Ok(())
}

fn bindings_list_output(
    config: &ClientConfigStore,
    registry: Option<&InputRegistry>,
) -> BindingsListOutput {
    BindingsListOutput {
        bindings: config
            .0
            .settings
            .input_bindings
            .iter()
            .map(binding_display)
            .collect(),
        supported_keys: supported_keys(),
        supported_slots: registry
            .into_iter()
            .flat_map(InputRegistry::slots)
            .map(|(slot, definition)| SupportedSlotDisplay {
                slot: slot.into(),
                display_name: definition.display_name.clone(),
            })
            .collect(),
    }
}

fn hud_show_output(hud: &HudCache) -> Result<HudShowOutput, serde_json::Error> {
    let target = hud.target.clone().map(serde_json::from_value).transpose()?;
    Ok(HudShowOutput {
        fps: HudFpsOutput {
            current: hud.fps.current,
            average: hud.fps.average,
            min: hud.fps.min,
            max: hud.fps.max,
        },
        position: hud.position.as_ref().map(|position| HudPositionOutput {
            absolute: position.absolute,
            chunk_relative: position.chunk_relative,
        }),
        target,
    })
}

macro_rules! definition {
    ($definition:ident, $input:ty, $output:ty, $name:literal, $level:literal) => {
        struct $definition;
        impl crate::ClientCommandDefinition for $definition {
            type Input = $input;
            type Output = $output;
            const NAME: &'static str = $name;
            const DEV_LEVEL: u8 = $level;
        }
    };
}
definition!(
    AppQuitDefinition,
    EmptyArguments,
    AcceptedOutput,
    "app.quit",
    0
);
definition!(
    OpenExternalUrlDefinition,
    OpenExternalUrlArguments,
    AcceptedOutput,
    "app.open-external-url",
    0
);
definition!(
    UiOpenDefinition,
    UiOpenArguments,
    UiOpenOutput,
    "ui.open",
    0
);
definition!(UiBackDefinition, UiBackArguments, EmptyOutput, "ui.back", 0);
definition!(
    CommandSchemaDefinition,
    CommandSchemaArguments,
    CommandSchemaOutput,
    "command.schema",
    1
);
definition!(
    CommandHelpDefinition,
    CommandHelpArguments,
    CommandHelpOutput,
    "command.help",
    0
);
definition!(DevDefinition, DevArguments, DevOutput, "dev", 0);
definition!(
    SettingsSetDefinition,
    SettingsSetArguments,
    EmptyOutput,
    "settings.set",
    0
);
definition!(
    ServerAddDefinition,
    ServerEntryArguments,
    ServerIndexOutput,
    "server.add",
    0
);
definition!(
    ServerEditDefinition,
    ServerEditArguments,
    ServerIndexOutput,
    "server.edit",
    0
);
definition!(
    ServerRefreshDefinition,
    EmptyArguments,
    ServerRefreshOutput,
    "server.refresh",
    0
);
definition!(
    ServerRetryDefinition,
    EmptyArguments,
    ServerStatusOutput,
    "server.retry",
    0
);
definition!(
    ServerDisconnectDefinition,
    EmptyArguments,
    ServerStatusOutput,
    "server.disconnect",
    0
);
definition!(
    ServerStatusDefinition,
    EmptyArguments,
    ServerStatusOutput,
    "server.status",
    0
);
definition!(
    SettingsShowDefinition,
    EmptyArguments,
    SettingsShowOutput,
    "settings.show",
    0
);
definition!(
    BindingsListDefinition,
    EmptyArguments,
    BindingsListOutput,
    "bindings.list",
    0
);
definition!(
    BindingsBindDefinition,
    BindingArguments,
    EmptyOutput,
    "bindings.bind",
    0
);
definition!(
    BindingsUnbindDefinition,
    BindingArguments,
    EmptyOutput,
    "bindings.unbind",
    0
);
definition!(
    BindingsReplaceDefinition,
    BindingsReplaceArguments,
    BindingMutationOutput,
    "bindings.replace",
    0
);
definition!(
    ServerListDefinition,
    EmptyArguments,
    ServerListOutput,
    "server.list",
    0
);
definition!(
    ServerDeleteDefinition,
    ServerIndexArguments,
    EmptyOutput,
    "server.delete",
    0
);
definition!(
    ServerConnectDefinition,
    ServerIndexArguments,
    ServerStatusOutput,
    "server.connect",
    0
);
definition!(
    DiagnosticsPositionDefinition,
    EmptyArguments,
    PositionOutput,
    "diagnostics.position",
    1
);
definition!(
    HudShowDefinition,
    EmptyArguments,
    HudShowOutput,
    "hud.show",
    1
);

/// 命令目录统一发现、schema 与 dispatch 注册，Unix adapter 只能投影这些 typed JSON 命令。
struct ClientCommandCatalogEntry {
    name: &'static str,
    dev_level: u8,
    schema: fn() -> serde_json::Value,
}

impl ClientCommandCatalogEntry {
    const fn of<D: ClientCommandDefinition>() -> Self {
        Self {
            name: D::NAME,
            dev_level: D::DEV_LEVEL,
            schema: crate::json_command::command_schema::<D>,
        }
    }
}

const CLIENT_COMMANDS: &[ClientCommandCatalogEntry] = &[
    ClientCommandCatalogEntry::of::<AppQuitDefinition>(),
    ClientCommandCatalogEntry::of::<OpenExternalUrlDefinition>(),
    ClientCommandCatalogEntry::of::<UiOpenDefinition>(),
    ClientCommandCatalogEntry::of::<UiBackDefinition>(),
    ClientCommandCatalogEntry::of::<ServerListDefinition>(),
    ClientCommandCatalogEntry::of::<ServerRefreshDefinition>(),
    ClientCommandCatalogEntry::of::<ServerAddDefinition>(),
    ClientCommandCatalogEntry::of::<ServerEditDefinition>(),
    ClientCommandCatalogEntry::of::<ServerDeleteDefinition>(),
    ClientCommandCatalogEntry::of::<ServerConnectDefinition>(),
    ClientCommandCatalogEntry::of::<ServerRetryDefinition>(),
    ClientCommandCatalogEntry::of::<ServerDisconnectDefinition>(),
    ClientCommandCatalogEntry::of::<ServerStatusDefinition>(),
    ClientCommandCatalogEntry::of::<SettingsShowDefinition>(),
    ClientCommandCatalogEntry::of::<SettingsSetDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsListDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsBindDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsUnbindDefinition>(),
    ClientCommandCatalogEntry::of::<BindingsReplaceDefinition>(),
    ClientCommandCatalogEntry::of::<DiagnosticsPositionDefinition>(),
    ClientCommandCatalogEntry::of::<HudShowDefinition>(),
    ClientCommandCatalogEntry::of::<CommandHelpDefinition>(),
    ClientCommandCatalogEntry::of::<CommandSchemaDefinition>(),
    ClientCommandCatalogEntry::of::<DevDefinition>(),
];

/// Owns command discovery, visibility, schemas, and dispatch completeness as
/// one deep module. Source adapters never inspect the declaration list.
struct ClientCommandCatalog;

const CLIENT_COMMAND_CATALOG: ClientCommandCatalog = ClientCommandCatalog;

impl ClientCommandCatalog {
    fn visible(&self, level: u8) -> Vec<&'static str> {
        CLIENT_COMMANDS
            .iter()
            .filter_map(|command| (command.dev_level <= level).then_some(command.name))
            .collect()
    }

    fn dev_level(&self, name: &str) -> Option<u8> {
        CLIENT_COMMANDS
            .iter()
            .find_map(|command| (command.name == name).then_some(command.dev_level))
    }

    fn schema(
        &self,
        command: &str,
    ) -> Result<serde_json::Value, crate::json_command::CommandError> {
        CLIENT_COMMANDS
            .iter()
            .find(|entry| entry.name == command)
            .map(|entry| (entry.schema)())
            .ok_or_else(|| match command.is_empty() {
                true => crate::json_command::CommandError::new(
                    "invalid_arguments",
                    "command must be a non-empty string",
                ),
                false => crate::json_command::CommandError::new(
                    "unknown_command",
                    "schema is unavailable",
                ),
            })
    }

    /// Refuses dispatch when composition omitted a catalog command. This check
    /// is owned here rather than repeated by Web UI and terminal adapters.
    fn assert_complete(&self, registry: &crate::json_command::CommandRegistry<'_, ()>) {
        let mut catalog_names = CLIENT_COMMANDS
            .iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        catalog_names.sort_unstable();
        assert_eq!(
            registry.registered_names(),
            catalog_names,
            "client command catalog and dispatch registrations diverged"
        );
    }
}

macro_rules! unix_empty_command {
    ($name:ident, $command:literal, $path:literal, $output:ty) => {
        #[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
        #[serde(deny_unknown_fields)]
        #[unix(path = $path)]
        struct $name {}
        impl ClientCommandDefinition for $name {
            type Input = Self;
            type Output = $output;
            const NAME: &'static str = $command;
            const DEV_LEVEL: u8 = 0;
        }
    };
}

unix_empty_command!(UnixAppQuit, "app.quit", "app quit", AcceptedOutput);
unix_empty_command!(UnixUiBack, "ui.back", "ui back", EmptyOutput);
unix_empty_command!(
    UnixServerList,
    "server.list",
    "server list",
    ServerListOutput
);
unix_empty_command!(
    UnixServerRefresh,
    "server.refresh",
    "server refresh",
    ServerRefreshOutput
);
unix_empty_command!(
    UnixServerRetry,
    "server.retry",
    "server retry",
    ServerStatusOutput
);
unix_empty_command!(
    UnixServerDisconnect,
    "server.disconnect",
    "server disconnect",
    ServerStatusOutput
);
unix_empty_command!(
    UnixServerStatus,
    "server.status",
    "server status",
    ServerStatusOutput
);
unix_empty_command!(
    UnixSettingsShow,
    "settings.show",
    "settings show",
    SettingsShowOutput
);
unix_empty_command!(
    UnixBindingsList,
    "bindings.list",
    "bindings list",
    BindingsListOutput
);
unix_empty_command!(
    UnixDiagnosticsPosition,
    "diagnostics.position",
    "diagnostics position",
    PositionOutput
);
unix_empty_command!(UnixHudShow, "hud.show", "hud show", HudShowOutput);

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "command help")]
struct UnixCommandHelp {
    command: Option<String>,
}
impl ClientCommandDefinition for UnixCommandHelp {
    type Input = Self;
    type Output = CommandHelpOutput;
    const NAME: &'static str = "command.help";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "command schema")]
struct UnixCommandSchema {
    command: String,
}
impl ClientCommandDefinition for UnixCommandSchema {
    type Input = Self;
    type Output = CommandSchemaOutput;
    const NAME: &'static str = "command.schema";
    const DEV_LEVEL: u8 = 1;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "dev")]
struct UnixDev {
    level: Option<u8>,
}
impl ClientCommandDefinition for UnixDev {
    type Input = Self;
    type Output = DevOutput;
    const NAME: &'static str = "dev";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "server add")]
struct UnixServerAdd {
    name: String,
    address: String,
}
impl ClientCommandDefinition for UnixServerAdd {
    type Input = Self;
    type Output = ServerIndexOutput;
    const NAME: &'static str = "server.add";
    const DEV_LEVEL: u8 = 0;
}

struct UnixServerEdit;
impl ClientCommandDefinition for UnixServerEdit {
    type Input = EmptyArguments;
    type Output = ServerIndexOutput;
    const NAME: &'static str = "server.edit";
    const DEV_LEVEL: u8 = 0;
}
impl UnixCommand for UnixServerEdit {
    const UNIX_PATH: &'static [&'static str] = &["server", "edit"];
    fn parse_unix(arguments: &[String]) -> Result<serde_json::Value, UnixCommandParseError> {
        if arguments.len() != 3 {
            return Err(UnixCommandParseError::new(
                "usage: server edit <index> <name> <address>",
            ));
        }
        let index = arguments[0]
            .parse::<usize>()
            .map_err(|_| UnixCommandParseError::new("invalid argument 1"))?;
        Ok(serde_json::json!({
            "index": index,
            "server": {
                "name": arguments[1],
                "address": arguments[2],
            }
        }))
    }
    fn usage() -> String {
        "server edit <index> <name> <address>".into()
    }
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "server delete")]
struct UnixServerDelete {
    index: usize,
}
impl ClientCommandDefinition for UnixServerDelete {
    type Input = Self;
    type Output = EmptyOutput;
    const NAME: &'static str = "server.delete";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "server connect")]
struct UnixServerConnect {
    index: usize,
}
impl ClientCommandDefinition for UnixServerConnect {
    type Input = Self;
    type Output = ServerStatusOutput;
    const NAME: &'static str = "server.connect";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "settings set")]
struct UnixSettingsSet {
    key: String,
    value: f32,
}
impl ClientCommandDefinition for UnixSettingsSet {
    type Input = Self;
    type Output = EmptyOutput;
    const NAME: &'static str = "settings.set";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "bindings bind")]
struct UnixBindingsBind {
    key: String,
    actions: Vec<String>,
}
impl ClientCommandDefinition for UnixBindingsBind {
    type Input = Self;
    type Output = EmptyOutput;
    const NAME: &'static str = "bindings.bind";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "bindings unbind")]
struct UnixBindingsUnbind {
    key: String,
    actions: Vec<String>,
}
impl ClientCommandDefinition for UnixBindingsUnbind {
    type Input = Self;
    type Output = EmptyOutput;
    const NAME: &'static str = "bindings.unbind";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Resource)]
struct ClientUnixAdapter {
    registry: UnixCommandRegistry,
}
impl Default for ClientUnixAdapter {
    fn default() -> Self {
        let mut registry = UnixCommandRegistry::default();
        registry.register::<UnixAppQuit>();
        registry.register::<UnixUiBack>();
        registry.register::<UnixCommandHelp>();
        registry.register::<UnixCommandSchema>();
        registry.register::<UnixDev>();
        registry.register::<UnixServerList>();
        registry.register::<UnixServerRefresh>();
        registry.register::<UnixServerAdd>();
        registry.register::<UnixServerEdit>();
        registry.register::<UnixServerDelete>();
        registry.register::<UnixServerConnect>();
        registry.register::<UnixServerRetry>();
        registry.register::<UnixServerDisconnect>();
        registry.register::<UnixServerStatus>();
        registry.register::<UnixSettingsShow>();
        registry.register::<UnixSettingsSet>();
        registry.register::<UnixBindingsList>();
        registry.register::<UnixBindingsBind>();
        registry.register::<UnixBindingsUnbind>();
        registry.register::<UnixDiagnosticsPosition>();
        registry.register::<UnixHudShow>();
        registry.register_alias::<UnixAppQuit>(&["quit"]);
        registry.register_alias::<UnixAppQuit>(&["exit"]);
        registry.register_alias::<UnixCommandHelp>(&["help"]);
        registry.register_alias::<UnixCommandHelp>(&["?"]);
        registry.register_alias::<UnixDiagnosticsPosition>(&["position"]);
        registry.register_alias::<UnixDiagnosticsPosition>(&["pos"]);
        Self { registry }
    }
}

/// Accepted terminal calls awaiting non-blocking response collection.
#[derive(Resource, Default)]
struct TerminalResponses(Vec<RequestCall<serde_json::Value>>);

/// Mutable in-memory client configuration used by command handlers.
///
/// Most handlers mutate this value before attempting persistence; callers must
/// inspect command errors because a save failure can leave memory and disk out
/// of sync.
#[derive(bevy::prelude::Resource)]
pub struct ClientConfigStore(pub ClientConfig);

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

impl ClientConfigStore {
    fn save(&self) -> Result<(), String> {
        roundo_user_config::save_config(CLIENT_CONFIG_FILE, &self.0)
    }
}

/// Accepts an absolute lowercase `http`/`https` URL without credentials or whitespace.
///
/// This is a narrow launch-safety check, not full URL parsing or host validation.
fn validate_external_url(value: &str) -> Result<&str, crate::json_command::CommandError> {
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(crate::json_command::CommandError::new(
            "invalid_external_url",
            "external URL contains whitespace or control characters",
        ));
    }
    let Some((scheme, remainder)) = value.split_once("://") else {
        return Err(crate::json_command::CommandError::new(
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
        return Err(crate::json_command::CommandError::new(
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

/// Installs missing client resources and the ordered command/data systems.
///
/// Preinserted command pipes and configuration stores are preserved.
pub(super) fn configure(app: &mut App) {
    if !app.world().contains_resource::<ClientCommandPipe>() {
        app.insert_resource(ClientCommandPipe::bounded(256));
    }
    if !app.world().contains_resource::<ClientConfigStore>() {
        app.insert_resource(ClientConfigStore(roundo_user_config::load_config(
            CLIENT_CONFIG_FILE,
        )));
    }
    app.init_resource::<DevLevel>();
    app.init_resource::<HudCache>();
    app.init_resource::<ClientDataSyncState>();
    app.init_resource::<ClientUnixAdapter>();
    app.init_resource::<TerminalResponses>();
    app.init_resource::<crate::client_network::ServerProbeManager>();
    app.add_systems(
        Update,
        (
            process_commands,
            process_json_commands,
            update_hud_cache,
            sync_subscribed_client_data,
        )
            .chain(),
    );
}

/// Drains source-neutral JSON requests on the Bevy main world. This is the
/// execution side of the Web UI/terminal command seam.
fn process_json_commands(
    pipe: Res<ClientCommandPipe>,
    mut config: ResMut<ClientConfigStore>,
    mut network: ResMut<crate::client_network::ClientNetworkManager>,
    probes: Res<crate::client_network::ServerProbeManager>,
    mut dev_level: ResMut<DevLevel>,
    controller: Res<ClientPlayerController>,
    cameras: Query<&Transform>,
    mut app_exit: MessageWriter<AppExit>,
    mut webui: Option<ResMut<roundo_webui::UiLifecycleManager>>,
    mut navigation: Option<bevy::ecs::system::NonSendMut<roundo_webui::UiNavigationExecutor>>,
    hud: Res<HudCache>,
    input_registry: Option<Res<InputRegistry>>,
) {
    for _ in 0..MAX_JSON_COMMANDS_PER_UPDATE {
        let Some((transport, reply)) = pipe.try_receive() else {
            break;
        };
        let request = transport.command;
        let ui_source = command_transport_source(transport.context);
        let command_value = &request["command"];
        let command_name = command_value.as_str().unwrap_or_default().to_owned();
        if command_name == "ui.open" {
            log::debug!("Processing Web UI ui.open command: {request}");
        }
        let response = application::ClientCommandApplication {
            config: &mut config,
            network: &mut network,
            probes: &probes,
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
            hud: &hud,
            input_registry: input_registry.as_deref(),
        }
        .dispatch(request, ui_source);
        let quit_accepted = response["command"] == "app.quit" && response["ok"] == true;
        if command_name == "ui.open" {
            if response["ok"] == true {
                log::debug!("Web UI ui.open command succeeded: {response}");
            } else {
                log::warn!("Web UI ui.open command failed: {response}");
            }
        }
        if command_name == "ui.open" && response["ok"] == true {
            let completed = navigation
                .as_deref_mut()
                .expect("successful UI Open has a navigation module")
                .complete_open_command(reply, response);
            assert!(
                completed.is_ok(),
                "successful UI Open has a pending transaction"
            );
            continue;
        }
        let source_was_destroyed = ui_source.is_some_and(|source| {
            !webui
                .as_deref()
                .is_some_and(|manager| manager.command_source_is_live(source))
        });
        let command_succeeded = response["ok"] == true;
        if reply.respond(response).is_err() {
            if command_succeeded && source_was_destroyed {
                log::debug!(
                    "Client command `{command_name}` destroyed its source before response delivery"
                );
            } else {
                log::warn!("Client command response receiver disconnected for `{command_name}`");
            }
        }
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
    serde_json::to_value(crate::json_command::CommandResult {
        version: crate::json_command::COMMAND_VERSION,
        command,
        ok: false,
        data: None,
        error: Some(crate::json_command::CommandError::new(
            "stale_ui_instance",
            "WebView command source is no longer live and loaded",
        )),
    })
    .expect("serializable stale UI command result")
}

/// Maps lifecycle-domain failures onto command error categories.
fn map_ui_lifecycle_error(
    error: roundo_webui::UiLifecycleError,
) -> crate::json_command::CommandError {
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
    crate::json_command::CommandError::new(code, error.to_string())
}

// Samples only the currently bound valid camera; missing cameras clear position.
fn update_hud_cache(
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    cameras: Query<&Transform>,
    mut hud: ResMut<HudCache>,
) {
    hud.record_frame(time.delta_secs());
    hud.position = controller
        .camera()
        .and_then(|entity| {
            let camera = cameras.get(entity);
            camera.ok()
        })
        .map(|transform| {
            let translation = transform.translation;
            HudPosition {
                absolute: [translation.x, translation.y, translation.z],
                chunk_relative: [
                    translation.x.floor() as i32,
                    translation.y.floor() as i32,
                    translation.z.floor() as i32,
                ],
            }
        });
}

/// Publishes changed/forced data and caches it only if at least one subscriber received it.
fn publish_client_data_if_changed(
    navigation: &mut roundo_webui::UiNavigationExecutor,
    sync: &mut ClientDataSyncState,
    resource: &'static str,
    snapshot: serde_json::Value,
    force: bool,
) {
    let changed = sync.last_snapshots.get(resource) != Some(&snapshot);
    if (force || changed) && navigation.publish_data(resource, &snapshot) != 0 {
        sync.last_snapshots.insert(resource, snapshot);
    }
}

/// Client Data is produced only on the client side. WebViews merely maintain
/// subscriptions; this system chooses event-driven or periodic synchronization
/// independently for each resource and never receives read requests over IPC.
fn sync_subscribed_client_data(
    time: Res<Time>,
    config: Res<ClientConfigStore>,
    network: Res<crate::client_network::ClientNetworkManager>,
    probes: Res<crate::client_network::ServerProbeManager>,
    hud: Res<HudCache>,
    input_registry: Option<Res<InputRegistry>>,
    mut sync: ResMut<ClientDataSyncState>,
    mut navigation: Option<bevy::ecs::system::NonSendMut<roundo_webui::UiNavigationExecutor>>,
) {
    let Some(navigation) = navigation.as_deref_mut() else {
        return;
    };
    let subscription_generation = navigation.data_subscription_generation();
    let subscriptions_changed = subscription_generation != sync.subscription_generation;
    sync.subscription_generation = subscription_generation;

    let server_list_revision = probes.change_revision();
    if navigation.has_data_subscribers(SERVER_LIST_DATA)
        && (subscriptions_changed || server_list_revision != sync.server_list_revision)
    {
        let snapshot = serde_json::to_value(server_list_output(&config, &probes))
            .expect("server list snapshot serializes");
        publish_client_data_if_changed(
            navigation,
            &mut sync,
            SERVER_LIST_DATA,
            snapshot,
            subscriptions_changed,
        );
        sync.server_list_revision = server_list_revision;
    }
    if navigation.has_data_subscribers(SERVER_STATUS_DATA) {
        let snapshot = serde_json::to_value(server_status_output(network.status()))
            .expect("server status snapshot serializes");
        publish_client_data_if_changed(
            navigation,
            &mut sync,
            SERVER_STATUS_DATA,
            snapshot,
            subscriptions_changed,
        );
    }
    if navigation.has_data_subscribers(SETTINGS_DATA) {
        let snapshot = serde_json::to_value(settings_show_output(&config))
            .expect("settings snapshot serializes");
        publish_client_data_if_changed(
            navigation,
            &mut sync,
            SETTINGS_DATA,
            snapshot,
            subscriptions_changed,
        );
    }
    if navigation.has_data_subscribers(BINDINGS_DATA) {
        let snapshot =
            serde_json::to_value(bindings_list_output(&config, input_registry.as_deref()))
                .expect("bindings snapshot serializes");
        publish_client_data_if_changed(
            navigation,
            &mut sync,
            BINDINGS_DATA,
            snapshot,
            subscriptions_changed,
        );
    }

    sync.hud_elapsed_secs += time.delta_secs();
    if navigation.has_data_subscribers(HUD_DATA)
        && (subscriptions_changed || sync.hud_elapsed_secs >= HUD_SYNC_INTERVAL_SECS)
    {
        sync.hud_elapsed_secs = 0.0;
        match hud_show_output(&hud).and_then(serde_json::to_value) {
            Ok(snapshot) => publish_client_data_if_changed(
                navigation,
                &mut sync,
                HUD_DATA,
                snapshot,
                subscriptions_changed,
            ),
            Err(error) => log::error!("cannot serialize HUD Client Data: {error}"),
        }
    }
}

// Collects prior terminal responses before admitting another bounded stdin batch.
fn process_commands(
    input: Res<TerminalInput>,
    adapter: Res<ClientUnixAdapter>,
    pipe: Res<ClientCommandPipe>,
    mut responses: ResMut<TerminalResponses>,
) {
    for response in collect_terminal_responses(&mut responses) {
        print_terminal_response(response);
    }
    for line in input.drain() {
        let Some(request) = project_terminal_line(&adapter.registry, &line) else {
            continue;
        };
        if let Some(response) = submit_terminal_request(&pipe, &mut responses, request) {
            print_terminal_response(response);
        }
    }
}

/// The terminal remains an opt-in Unix projection: it tokenizes text, projects
/// it to a versioned JSON envelope, and only then crosses the command pipe.
fn project_terminal_line(registry: &UnixCommandRegistry, line: &str) -> Option<serde_json::Value> {
    let tokens = match crate::json_command::tokenize_unix_line(line) {
        Ok(tokens) if tokens.is_empty() => return None,
        Ok(tokens) => tokens,
        Err(error) => return Some(terminal_error("", "invalid_command_envelope", error)),
    };
    Some(match registry.project(&tokens) {
        Ok((command, arguments)) => serde_json::json!({
            "version": crate::json_command::COMMAND_VERSION,
            "command": command,
            "arguments": arguments,
        }),
        Err(error) => terminal_error(
            "",
            if error.is_unknown_command() {
                "unknown_command"
            } else {
                "invalid_arguments"
            },
            error,
        ),
    })
}

/// Attempts non-blocking host-authority admission to the shared JSON queue.
fn submit_terminal_request(
    pipe: &ClientCommandPipe,
    responses: &mut TerminalResponses,
    request: serde_json::Value,
) -> Option<serde_json::Value> {
    match pipe
        .io()
        .submit(request, roundo_webui::UiCommandSource::Host)
    {
        Ok(call) => {
            responses.0.push(call);
            None
        }
        Err(JsonSubmitError::Full) => Some(terminal_error(
            "",
            "command_queue_full",
            "command queue is full",
        )),
        Err(JsonSubmitError::Disconnected) => Some(terminal_error(
            "",
            "internal_command_error",
            "command queue is unavailable",
        )),
        Err(JsonSubmitError::InputTooLarge { command }) => Some(terminal_error(
            &command,
            "command_input_too_large",
            "command input exceeds 64 KiB",
        )),
    }
}

/// Partitions calls into completed responses and handles still reporting `None`.
///
/// `RequestCall::try_result` conflates pending and disconnected response paths,
/// so a disconnected call remains retained by this polling adapter.
fn collect_terminal_responses(responses: &mut TerminalResponses) -> Vec<serde_json::Value> {
    let mut pending = Vec::new();
    let mut completed = Vec::new();
    for call in std::mem::take(&mut responses.0) {
        match call.try_result() {
            Some(response) => completed.push(response),
            None => pending.push(call),
        }
    }
    responses.0 = pending;
    completed
}

fn terminal_error(command: &str, code: &str, message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({
        "version": crate::json_command::COMMAND_VERSION,
        "command": command,
        "ok": false,
        "error": {"code": code, "message": message.to_string()},
    })
}

fn print_terminal_response(response: serde_json::Value) {
    println!("{response}");
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_COMMAND_CATALOG, CLIENT_COMMANDS, UiOpenArguments, validate_external_url};

    fn assert_has_property(value: &serde_json::Value, name: &str) {
        let property = value.get(name);
        assert!(property.is_some(), "schema property `{name}` is missing");
    }

    #[test]
    fn ui_open_accepts_only_a_source_local_import_handle() {
        assert!(
            serde_json::from_value::<UiOpenArguments>(serde_json::json!({
                "import": "settings"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<UiOpenArguments>(serde_json::json!({
                "slot": "roundo.settings"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<UiOpenArguments>(serde_json::json!({
                "resource": "vanilla.vanilla_ui.settings"
            }))
            .is_err()
        );
    }

    #[test]
    fn catalog_has_unique_names_and_a_schema_for_every_typed_command() {
        let mut names = std::collections::BTreeSet::new();
        for entry in CLIENT_COMMANDS {
            assert!(
                names.insert(entry.name),
                "duplicate command: {}",
                entry.name
            );
            let schema = (entry.schema)();
            assert_eq!(schema["command"], entry.name);
        }
        assert_eq!(names.len(), CLIENT_COMMANDS.len());
    }

    #[test]
    fn dev_level_filters_discovery_but_not_the_catalog() {
        assert!(!CLIENT_COMMAND_CATALOG.visible(0).contains(&"hud.show"));
        assert!(CLIENT_COMMAND_CATALOG.visible(1).contains(&"hud.show"));
        assert_eq!(CLIENT_COMMAND_CATALOG.dev_level("hud.show"), Some(1));
        assert_eq!(CLIENT_COMMAND_CATALOG.dev_level("missing"), None);
    }

    #[test]
    fn external_url_policy_accepts_only_http_without_credentials() {
        assert_eq!(
            validate_external_url("https://example.test/path").unwrap(),
            "https://example.test/path"
        );
        for invalid in [
            "javascript:alert(1)",
            "file:///tmp/a",
            "https://user@example.test/",
            " https://example.test/",
            "https://",
        ] {
            assert!(validate_external_url(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn typed_hud_and_position_definitions_expose_empty_argument_schemas() {
        for schema in [
            crate::json_command::command_schema::<super::HudShowDefinition>(),
            crate::json_command::command_schema::<super::DiagnosticsPositionDefinition>(),
        ] {
            assert_eq!(schema["input"]["properties"]["arguments"]["type"], "object");
            assert_eq!(
                schema["input"]["properties"]["arguments"]["additionalProperties"],
                false
            );
        }
    }

    #[test]
    fn success_schemas_describe_client_display_status_settings_bindings_and_meta_outputs() {
        let status = CLIENT_COMMAND_CATALOG.schema("server.status").unwrap();
        let properties = &status["success"]["properties"]["data"]["properties"];
        assert_has_property(properties, "status");
        let status_schema = &properties["status"];
        assert_has_property(status_schema, "$ref");

        for command in ["server.list", "settings.show", "bindings.list"] {
            let schema = CLIENT_COMMAND_CATALOG.schema(command).unwrap();
            assert_eq!(schema["success"]["properties"]["data"]["type"], "object");
        }
        let list = CLIENT_COMMAND_CATALOG.schema("server.list").unwrap();
        let properties = &list["success"]["properties"]["data"]["properties"];
        assert_has_property(properties, "servers");
        let settings = CLIENT_COMMAND_CATALOG.schema("settings.show").unwrap();
        let properties = &settings["success"]["properties"]["data"]["properties"];
        assert_has_property(properties, "settings");
        let bindings = CLIENT_COMMAND_CATALOG.schema("bindings.list").unwrap();
        let properties = &bindings["success"]["properties"]["data"]["properties"];
        assert_has_property(properties, "supported_keys");
        let meta = CLIENT_COMMAND_CATALOG.schema("command.help").unwrap();
        assert_has_property(&meta["success"]["properties"]["data"], "anyOf");
        let schema_meta = CLIENT_COMMAND_CATALOG.schema("command.schema").unwrap();
        assert_eq!(
            schema_meta["success"]["properties"]["data"]["type"],
            "object"
        );
    }

    #[test]
    fn binding_input_rejects_empty_slots_and_unknown_keys() {
        let valid = super::BindingArguments {
            slot: "roundo.move-forward".into(),
            key: "key_w".into(),
        };
        assert!(roundo_user_config::ClientInputBindingConfig::try_from(valid).is_ok());
        for arguments in [
            super::BindingArguments {
                slot: "".into(),
                key: "key_w".into(),
            },
            super::BindingArguments {
                slot: "roundo.move-forward".into(),
                key: "escape".into(),
            },
            super::BindingArguments {
                slot: "roundo.move-forward".into(),
                key: "unknown".into(),
            },
        ] {
            assert!(roundo_user_config::ClientInputBindingConfig::try_from(arguments).is_err());
        }
    }

    #[test]
    fn bindings_list_display_excludes_escape_and_includes_mouse() {
        let keys = super::supported_keys();
        assert_eq!(keys.len(), 55);
        assert_eq!(keys[0].key, "digit0");
        assert_eq!(keys.last().unwrap().key, "mouse_middle");
        assert!(
            keys.iter()
                .all(|key| key.key != "escape" && !key.display_name.is_empty())
        );

        let binding = roundo_user_config::ClientInputBindingConfig::new(
            "roundo.move-forward",
            roundo_user_config::ClientInputKey::KeyW,
        );
        let display = super::binding_display(&binding);
        assert_eq!(display.slot, "roundo.move-forward");
        assert_eq!(display.key, "key_w");
    }

    #[test]
    fn bindings_replace_input_is_strict_and_reuses_binding_validation() {
        let valid = serde_json::json!({
            "old_binding": {"slot": "roundo.move-forward", "key": "key_w"},
            "binding": {"slot": "roundo.move-forward", "key": "key_e"},
        });
        let parsed: super::BindingsReplaceArguments = serde_json::from_value(valid).unwrap();
        assert!(roundo_user_config::ClientInputBindingConfig::try_from(parsed.old_binding).is_ok());
        assert!(roundo_user_config::ClientInputBindingConfig::try_from(parsed.binding).is_ok());
        assert!(
            serde_json::from_value::<super::BindingsReplaceArguments>(serde_json::json!({
                "old_binding": {"slot": "roundo.move-forward", "key": "key_w"},
                "binding": {"slot": "roundo.move-forward", "key": "key_e"},
                "unexpected": true,
            }))
            .is_err()
        );
    }

    #[test]
    fn bindings_replace_is_atomic_and_rejects_missing_or_duplicate_edges() {
        use roundo_user_config::{ClientInputBindingConfig, ClientInputKey};
        let old = ClientInputBindingConfig::new("roundo.move-forward", ClientInputKey::KeyW);
        let replacement =
            ClientInputBindingConfig::new("roundo.move-forward", ClientInputKey::KeyE);
        let mut bindings = vec![old.clone()];
        super::replace_binding(&mut bindings, old.clone(), replacement.clone()).unwrap();
        assert_eq!(bindings, vec![replacement.clone()]);
        let before_missing = bindings.clone();
        let missing = ClientInputBindingConfig::new("roundo.move-left", ClientInputKey::KeyQ);
        let error = super::replace_binding(&mut bindings, missing, old.clone()).unwrap_err();
        assert_eq!(error.code, "binding_not_found");
        assert_eq!(bindings, before_missing);
        bindings.push(old.clone());
        let before_duplicate = bindings.clone();
        let error = super::replace_binding(&mut bindings, replacement, old).unwrap_err();
        assert_eq!(error.code, "binding_exists");
        assert_eq!(bindings, before_duplicate);
    }

    #[test]
    fn terminal_projects_unix_paths_aliases_and_quoted_arguments_to_envelopes() {
        let adapter = super::ClientUnixAdapter::default();
        assert_eq!(
            super::project_terminal_line(
                &adapter.registry,
                "server add 'Local Server' 127.0.0.1:5000",
            )
            .unwrap(),
            serde_json::json!({
                "version": 1,
                "command": "server.add",
                "arguments": {
                    "name": "Local Server",
                    "address": "127.0.0.1:5000",
                },
            })
        );
        assert_eq!(
            super::project_terminal_line(&adapter.registry, "pos").unwrap()["command"],
            "diagnostics.position"
        );
        assert_eq!(
            super::project_terminal_line(&adapter.registry, "quit").unwrap()["command"],
            "app.quit"
        );
    }

    #[test]
    fn terminal_projects_server_edit_to_the_existing_nested_json_contract() {
        let adapter = super::ClientUnixAdapter::default();
        let request =
            super::project_terminal_line(&adapter.registry, "server edit 2 Local 127.0.0.1:5000")
                .unwrap();
        assert_eq!(request["command"], "server.edit");
        assert_eq!(request["arguments"]["index"], 2);
        assert_eq!(request["arguments"]["server"]["name"], "Local");
    }

    #[test]
    fn terminal_projection_errors_and_queue_full_are_typed_json() {
        let adapter = super::ClientUnixAdapter::default();
        let unknown = super::project_terminal_line(&adapter.registry, "unknown command").unwrap();
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"]["code"], "unknown_command");

        let pipe = crate::ClientCommandPipe::bounded(1);
        let _occupied = pipe
            .io()
            .submit(serde_json::json!({}), roundo_webui::UiCommandSource::Host)
            .unwrap();
        let mut responses = super::TerminalResponses::default();
        let response =
            super::submit_terminal_request(&pipe, &mut responses, serde_json::json!({})).unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "command_queue_full");
    }

    #[test]
    fn terminal_rejects_oversized_serialized_requests_before_queueing() {
        let pipe = crate::ClientCommandPipe::bounded(1);
        let mut responses = super::TerminalResponses::default();
        let response = super::submit_terminal_request(
            &pipe,
            &mut responses,
            serde_json::json!({
                "version": 1,
                "command": "app.quit",
                "arguments": {"payload": "x".repeat(64 * 1024)},
            }),
        )
        .unwrap();
        assert_eq!(response["version"], 1);
        assert_eq!(response["command"], "app.quit");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "command_input_too_large");
        assert!(pipe.try_receive().is_none());
    }

    #[test]
    fn command_transport_context_keeps_host_and_webview_authority_out_of_json() {
        assert_eq!(
            super::command_transport_source(roundo_webui::UiCommandSource::Host),
            None
        );
        assert_eq!(
            super::command_transport_source(roundo_webui::UiCommandSource::WebView(
                roundo_webui::UiInstanceId::from_host_id(42),
            ))
            .map(roundo_webui::UiInstanceId::value),
            Some(42)
        );
    }

    #[test]
    fn stale_webview_source_returns_a_typed_result_without_mutating_the_command() {
        let command = serde_json::json!({
            "version": 1,
            "command": "ui.back",
            "arguments": {},
        });
        let response = super::stale_ui_source_response(&command);
        assert_eq!(response["command"], "ui.back");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "stale_ui_instance");
        assert_eq!(command["arguments"], serde_json::json!({}));
        let injected_source = command.get("_roundo_source_instance");
        assert!(injected_source.is_none());
    }

    #[test]
    fn terminal_collects_the_unmodified_typed_json_response() {
        let pipe = crate::ClientCommandPipe::bounded(1);
        let mut responses = super::TerminalResponses::default();
        assert!(
            super::submit_terminal_request(
                &pipe,
                &mut responses,
                serde_json::json!({"version": 1, "command": "app.quit", "arguments": {}}),
            )
            .is_none()
        );
        let (_, reply) = pipe.try_receive().unwrap();
        let expected = serde_json::json!({
            "version": 1,
            "command": "app.quit",
            "ok": true,
            "data": {"status": "accepted"},
        });
        reply.respond(expected.clone()).unwrap();
        assert_eq!(
            super::collect_terminal_responses(&mut responses),
            vec![expected]
        );
    }
}
