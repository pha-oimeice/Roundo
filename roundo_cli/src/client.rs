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
use roundo_marionette::ClientPlayerController;
use roundo_toolbox::request_response_pipe::{JsonSubmitError, RequestCall};
use roundo_user_config::{
    ClientConfig, ClientKeyBindingConfig, ClientKeyCode, ClientMovementAction, ServerEntry,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::VecDeque, process::Command};

const CLIENT_CONFIG_FILE: &str = "roundo-client-config.toml";
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
    slot: Option<String>,
    resource: Option<String>,
    path: Option<String>,
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
    key: String,
    actions: Vec<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BindingsReplaceArguments {
    old_binding: BindingArguments,
    binding: BindingArguments,
}
impl TryFrom<BindingArguments> for ClientKeyBindingConfig {
    type Error = crate::json_command::CommandError;
    fn try_from(value: BindingArguments) -> Result<Self, Self::Error> {
        if value.actions.is_empty() {
            return Err(crate::json_command::CommandError::new(
                "invalid_arguments",
                "binding actions cannot be empty",
            ));
        }
        let unique_count = value
            .actions
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if unique_count != value.actions.len() {
            return Err(crate::json_command::CommandError::new(
                "invalid_arguments",
                "binding actions must be unique",
            ));
        }
        serde_json::from_value(serde_json::json!({"key": value.key, "actions": value.actions}))
            .map_err(|error| {
                crate::json_command::CommandError::new("invalid_arguments", error.to_string())
            })
    }
}

/// Stable UI-facing binding relation, adapted from the canonical config enums.
#[derive(Serialize, JsonSchema)]
struct BindingDisplay {
    key: String,
    actions: Vec<String>,
}
#[derive(Serialize, JsonSchema)]
struct SupportedKeyDisplay {
    key: String,
    display_name: String,
}
#[derive(Serialize, JsonSchema)]
struct SupportedActionDisplay {
    action: String,
    display_name: String,
}
#[derive(Serialize, JsonSchema)]
struct BindingsListOutput {
    bindings: Vec<BindingDisplay>,
    supported_keys: Vec<SupportedKeyDisplay>,
    supported_actions: Vec<SupportedActionDisplay>,
}
#[derive(Serialize, JsonSchema)]
struct BindingMutationOutput {}

fn key_display(key: ClientKeyCode) -> SupportedKeyDisplay {
    let (key, display_name) = match key {
        ClientKeyCode::Escape => ("escape", "Escape"),
        ClientKeyCode::Digit0 => ("digit0", "0"),
        ClientKeyCode::Digit1 => ("digit1", "1"),
        ClientKeyCode::Digit2 => ("digit2", "2"),
        ClientKeyCode::Digit3 => ("digit3", "3"),
        ClientKeyCode::Digit4 => ("digit4", "4"),
        ClientKeyCode::Digit5 => ("digit5", "5"),
        ClientKeyCode::Digit6 => ("digit6", "6"),
        ClientKeyCode::Digit7 => ("digit7", "7"),
        ClientKeyCode::Digit8 => ("digit8", "8"),
        ClientKeyCode::Digit9 => ("digit9", "9"),
        ClientKeyCode::Backspace => ("backspace", "Backspace"),
        ClientKeyCode::Tab => ("tab", "Tab"),
        ClientKeyCode::KeyQ => ("key_q", "Q"),
        ClientKeyCode::KeyW => ("key_w", "W"),
        ClientKeyCode::KeyE => ("key_e", "E"),
        ClientKeyCode::KeyR => ("key_r", "R"),
        ClientKeyCode::KeyT => ("key_t", "T"),
        ClientKeyCode::KeyY => ("key_y", "Y"),
        ClientKeyCode::KeyU => ("key_u", "U"),
        ClientKeyCode::KeyI => ("key_i", "I"),
        ClientKeyCode::KeyO => ("key_o", "O"),
        ClientKeyCode::KeyP => ("key_p", "P"),
        ClientKeyCode::CapsLock => ("caps_lock", "Caps Lock"),
        ClientKeyCode::KeyA => ("key_a", "A"),
        ClientKeyCode::KeyS => ("key_s", "S"),
        ClientKeyCode::KeyD => ("key_d", "D"),
        ClientKeyCode::KeyF => ("key_f", "F"),
        ClientKeyCode::KeyG => ("key_g", "G"),
        ClientKeyCode::KeyH => ("key_h", "H"),
        ClientKeyCode::KeyJ => ("key_j", "J"),
        ClientKeyCode::KeyK => ("key_k", "K"),
        ClientKeyCode::KeyL => ("key_l", "L"),
        ClientKeyCode::Enter => ("enter", "Enter"),
        ClientKeyCode::ShiftLeft => ("shift_left", "Left Shift"),
        ClientKeyCode::KeyZ => ("key_z", "Z"),
        ClientKeyCode::KeyX => ("key_x", "X"),
        ClientKeyCode::KeyC => ("key_c", "C"),
        ClientKeyCode::KeyV => ("key_v", "V"),
        ClientKeyCode::KeyB => ("key_b", "B"),
        ClientKeyCode::KeyN => ("key_n", "N"),
        ClientKeyCode::KeyM => ("key_m", "M"),
        ClientKeyCode::ShiftRight => ("shift_right", "Right Shift"),
        ClientKeyCode::ControlLeft => ("control_left", "Left Control"),
        ClientKeyCode::AltLeft => ("alt_left", "Left Alt"),
        ClientKeyCode::Space => ("space", "Space"),
        ClientKeyCode::AltRight => ("alt_right", "Right Alt"),
        ClientKeyCode::ControlRight => ("control_right", "Right Control"),
        ClientKeyCode::ArrowLeft => ("arrow_left", "Left Arrow"),
        ClientKeyCode::ArrowUp => ("arrow_up", "Up Arrow"),
        ClientKeyCode::ArrowDown => ("arrow_down", "Down Arrow"),
        ClientKeyCode::ArrowRight => ("arrow_right", "Right Arrow"),
    };
    SupportedKeyDisplay {
        key: key.into(),
        display_name: display_name.into(),
    }
}

fn action_display(action: ClientMovementAction) -> SupportedActionDisplay {
    let (action, display_name) = match action {
        ClientMovementAction::MoveUp => ("move_up", "Move Up"),
        ClientMovementAction::MoveDown => ("move_down", "Move Down"),
        ClientMovementAction::MoveLeft => ("move_left", "Move Left"),
        ClientMovementAction::MoveRight => ("move_right", "Move Right"),
        ClientMovementAction::MoveForward => ("move_forward", "Move Forward"),
        ClientMovementAction::MoveBackward => ("move_backward", "Move Backward"),
    };
    SupportedActionDisplay {
        action: action.into(),
        display_name: display_name.into(),
    }
}

fn supported_keys() -> Vec<SupportedKeyDisplay> {
    [
        ClientKeyCode::Escape,
        ClientKeyCode::Digit0,
        ClientKeyCode::Digit1,
        ClientKeyCode::Digit2,
        ClientKeyCode::Digit3,
        ClientKeyCode::Digit4,
        ClientKeyCode::Digit5,
        ClientKeyCode::Digit6,
        ClientKeyCode::Digit7,
        ClientKeyCode::Digit8,
        ClientKeyCode::Digit9,
        ClientKeyCode::Backspace,
        ClientKeyCode::Tab,
        ClientKeyCode::KeyQ,
        ClientKeyCode::KeyW,
        ClientKeyCode::KeyE,
        ClientKeyCode::KeyR,
        ClientKeyCode::KeyT,
        ClientKeyCode::KeyY,
        ClientKeyCode::KeyU,
        ClientKeyCode::KeyI,
        ClientKeyCode::KeyO,
        ClientKeyCode::KeyP,
        ClientKeyCode::CapsLock,
        ClientKeyCode::KeyA,
        ClientKeyCode::KeyS,
        ClientKeyCode::KeyD,
        ClientKeyCode::KeyF,
        ClientKeyCode::KeyG,
        ClientKeyCode::KeyH,
        ClientKeyCode::KeyJ,
        ClientKeyCode::KeyK,
        ClientKeyCode::KeyL,
        ClientKeyCode::Enter,
        ClientKeyCode::ShiftLeft,
        ClientKeyCode::KeyZ,
        ClientKeyCode::KeyX,
        ClientKeyCode::KeyC,
        ClientKeyCode::KeyV,
        ClientKeyCode::KeyB,
        ClientKeyCode::KeyN,
        ClientKeyCode::KeyM,
        ClientKeyCode::ShiftRight,
        ClientKeyCode::ControlLeft,
        ClientKeyCode::AltLeft,
        ClientKeyCode::Space,
        ClientKeyCode::AltRight,
        ClientKeyCode::ControlRight,
        ClientKeyCode::ArrowLeft,
        ClientKeyCode::ArrowUp,
        ClientKeyCode::ArrowDown,
        ClientKeyCode::ArrowRight,
    ]
    .into_iter()
    .map(key_display)
    .collect()
}

fn supported_actions() -> Vec<SupportedActionDisplay> {
    [
        ClientMovementAction::MoveUp,
        ClientMovementAction::MoveDown,
        ClientMovementAction::MoveLeft,
        ClientMovementAction::MoveRight,
        ClientMovementAction::MoveForward,
        ClientMovementAction::MoveBackward,
    ]
    .into_iter()
    .map(action_display)
    .collect()
}

fn binding_display(binding: &ClientKeyBindingConfig) -> BindingDisplay {
    BindingDisplay {
        key: key_display(binding.key).key,
        actions: binding
            .actions
            .iter()
            .copied()
            .map(|action| action_display(action).action)
            .collect(),
    }
}

fn replace_binding(
    bindings: &mut Vec<ClientKeyBindingConfig>,
    old_binding: ClientKeyBindingConfig,
    replacement: ClientKeyBindingConfig,
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
        .any(|(existing_index, existing)| {
            existing_index != index && existing.key == replacement.key
        })
    {
        return Err(crate::json_command::CommandError::new(
            "binding_exists",
            "replacement key already has a binding",
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

fn visible_commands(level: u8) -> Vec<&'static str> {
    CLIENT_COMMANDS
        .iter()
        .filter_map(|command| (command.dev_level <= level).then_some(command.name))
        .collect()
}

fn command_dev_level(name: &str) -> Option<u8> {
    CLIENT_COMMANDS
        .iter()
        .find_map(|command| (command.name == name).then_some(command.dev_level))
}

fn client_command_schema(
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
            false => {
                crate::json_command::CommandError::new("unknown_command", "schema is unavailable")
            }
        })
}

/// composition 遗漏目录命令时拒绝 dispatch，防止 catalog、schema 与 handler 漂移。
fn assert_catalog_matches_registry(registry: &crate::json_command::CommandRegistry<'_, ()>) {
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
#[unix(path = "ui open")]
struct UnixUiOpen {
    #[unix(long)]
    slot: Option<String>,
    #[unix(long)]
    resource: Option<String>,
    #[unix(long)]
    path: Option<String>,
}
impl ClientCommandDefinition for UnixUiOpen {
    type Input = Self;
    type Output = UiOpenOutput;
    const NAME: &'static str = "ui.open";
    const DEV_LEVEL: u8 = 0;
}

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
        registry.register::<UnixUiOpen>();
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

#[derive(Resource, Default)]
struct TerminalResponses(Vec<RequestCall<serde_json::Value>>);

#[derive(bevy::prelude::Resource)]
pub struct ClientConfigStore(pub ClientConfig);

/// Read-only diagnostic cache consumed by the HUD command. Producers update
/// this resource; Web UI only crosses the small `hud.show` command seam.
#[derive(Resource, Clone, Debug, Serialize)]
pub struct HudCache {
    pub fps: HudFps,
    pub position: Option<HudPosition>,
    pub target: Option<serde_json::Value>,
    #[serde(skip)]
    frames: VecDeque<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HudFps {
    pub current: f32,
    pub average: f32,
    pub min: f32,
    pub max: f32,
}
#[derive(Clone, Debug, Serialize)]
pub struct HudPosition {
    pub absolute: [f32; 3],
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
    app.init_resource::<ClientUnixAdapter>();
    app.init_resource::<TerminalResponses>();
    app.init_resource::<crate::client_network::ServerProbeManager>();
    app.add_systems(
        Update,
        (process_commands, process_json_commands, update_hud_cache).chain(),
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
) {
    for _ in 0..MAX_JSON_COMMANDS_PER_UPDATE {
        let Some((mut request, reply)) = pipe.try_receive() else {
            break;
        };
        let ui_source = request
            .as_object_mut()
            .and_then(|object| object.remove("_roundo_source_instance"))
            .and_then(|value| value.as_u64())
            .map(roundo_webui::UiInstanceId::from_host_id);
        let command_name = request
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if command_name == "ui.open" {
            log::debug!("Processing Web UI ui.open command: {request}");
            if let Some(executor) = navigation.as_deref_mut() {
                executor.prepare_open_response();
            }
        }
        let response = dispatch_typed_command(
            request,
            &mut config,
            &mut network,
            &probes,
            &mut dev_level,
            &controller,
            &cameras,
            webui.as_deref_mut(),
            navigation.as_deref_mut(),
            ui_source,
            &hud,
        );
        let quit_accepted = response["command"] == "app.quit" && response["ok"] == true;
        if command_name == "ui.open" {
            if response["ok"] == true {
                log::debug!("Web UI ui.open command succeeded: {response}");
            } else {
                log::warn!("Web UI ui.open command failed: {response}");
            }
        }
        if command_name == "ui.open" && response["ok"] == true {
            if let Some((executor, pending)) = navigation
                .as_deref_mut()
                .and_then(|executor| executor.take_last_open().map(|pending| (executor, pending)))
            {
                executor.defer_open_response(pending, reply, response);
                continue;
            }
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
            app_exit.write(AppExit::Success);
        }
    }
}

/// Registers the typed definitions at the Bevy main-world seam. The registry
/// owns envelope/input/output mechanics; this adapter only supplies the state
/// that existing client capabilities require.
fn map_ui_lifecycle_error(
    error: roundo_webui::UiLifecycleError,
) -> crate::json_command::CommandError {
    let code = match error {
        roundo_webui::UiLifecycleError::UiInstanceLimit => "ui_instance_limit",
        roundo_webui::UiLifecycleError::DuplicatePendingOpen => "ui_open_pending",
        roundo_webui::UiLifecycleError::StaleUiInstance => "stale_ui_instance",
        roundo_webui::UiLifecycleError::CannotCloseUiRoot => "cannot_close_ui_root",
        roundo_webui::UiLifecycleError::Registry(_) => "invalid_ui_resource",
        roundo_webui::UiLifecycleError::NavigationFailed(_) => "ui_navigation_failed",
        roundo_webui::UiLifecycleError::LoadTimeout => "ui_load_timeout",
        roundo_webui::UiLifecycleError::InternalTree(_) => "internal_ui_error",
    };
    crate::json_command::CommandError::new(code, error.to_string())
}

fn dispatch_typed_command(
    request: serde_json::Value,
    config: &mut ClientConfigStore,
    network: &mut crate::client_network::ClientNetworkManager,
    probes: &crate::client_network::ServerProbeManager,
    dev_level: &mut DevLevel,
    controller: &ClientPlayerController,
    cameras: &Query<&Transform>,
    webui: Option<&mut roundo_webui::UiLifecycleManager>,
    navigation: Option<&mut roundo_webui::UiNavigationExecutor>,
    ui_source: Option<roundo_webui::UiInstanceId>,
    hud: &HudCache,
) -> serde_json::Value {
    // Admission is deliberately outside individual handlers: every command
    // arriving from a WebView must prove that its host-bound source endpoint
    // is still committed, loaded, enabled, and not destroying before any
    // business service can accept it. Visibility and focus are irrelevant.
    if let Some(source) = ui_source {
        let admitted = webui
            .as_deref()
            .is_some_and(|manager| manager.command_source_is_live(source));
        if !admitted {
            let command = request
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return serde_json::to_value(crate::json_command::CommandResult {
                version: crate::json_command::COMMAND_VERSION,
                command,
                ok: false,
                data: None,
                error: Some(crate::json_command::CommandError::new(
                    "stale_ui_instance",
                    "WebView command source is no longer live and loaded",
                )),
            })
            .expect("serializable stale UI command result");
        }
    }

    let config = RefCell::new(config);
    let network = RefCell::new(network);
    let dev_level = RefCell::new(dev_level);
    let webui = RefCell::new(webui);
    let navigation = RefCell::new(navigation);
    let mut registry = crate::json_command::CommandRegistry::default();
    registry.register_typed::<ServerConnectDefinition>(|input, _| {
        let config = config.borrow();
        let mut network = network.borrow_mut();
        let server = config.0.servers.get(input.index).ok_or_else(|| {
            crate::json_command::CommandError::new(
                "index_out_of_range",
                "server index does not exist",
            )
        })?;
        network
            .connect(server)
            .map_err(|error| crate::json_command::CommandError::new("connection_failed", error))?;
        Ok(server_status_output(network.status()))
    });
    registry.register_typed::<ServerRetryDefinition>(|_, _| {
        let mut network = network.borrow_mut();
        network
            .retry()
            .map_err(|error| crate::json_command::CommandError::new("no_active_server", error))?;
        Ok(server_status_output(network.status()))
    });
    registry.register_typed::<ServerDisconnectDefinition>(|_, _| {
        let mut network = network.borrow_mut();
        network.disconnect();
        Ok(server_status_output(network.status()))
    });
    registry.register_typed::<ServerStatusDefinition>(|_, _| {
        Ok(server_status_output(network.borrow().status()))
    });
    registry.register_typed::<ServerRefreshDefinition>(|_, _| {
        let revision = probes.refresh(&config.borrow().0.servers);
        Ok(ServerRefreshOutput {
            accepted: true,
            revision,
        })
    });
    registry.register_typed::<ServerListDefinition>(|_, _| {
        let config = config.borrow();
        let servers = config
            .0
            .servers
            .iter()
            .enumerate()
            .map(|(index, server)| ServerListEntryOutput {
                index,
                name: server.name.clone(),
                address: server.address.clone(),
                probe: probe_output(probes.result(index)),
            })
            .collect();
        Ok(ServerListOutput { servers })
    });
    registry.register_typed::<ServerAddDefinition>(|input, _| {
        let server = ServerEntry::from(input);
        if server.name.trim().is_empty() || server.address.trim().is_empty() {
            return Err(crate::json_command::CommandError::new(
                "invalid_arguments",
                "name and address are required",
            ));
        }
        let mut config = config.borrow_mut();
        config.0.servers.push(server);
        config
            .save()
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        Ok(ServerIndexOutput {
            index: config.0.servers.len() - 1,
        })
    });
    registry.register_typed::<ServerEditDefinition>(|input, _| {
        let server = ServerEntry::from(input.server);
        if server.name.trim().is_empty() || server.address.trim().is_empty() {
            return Err(crate::json_command::CommandError::new(
                "invalid_arguments",
                "index and a complete server are required",
            ));
        }
        let mut config = config.borrow_mut();
        if input.index >= config.0.servers.len() {
            return Err(crate::json_command::CommandError::new(
                "index_out_of_range",
                "server index does not exist",
            ));
        }
        config.0.servers[input.index] = server;
        config
            .save()
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        Ok(ServerIndexOutput { index: input.index })
    });
    registry.register_typed::<ServerDeleteDefinition>(|input, _| {
        let mut config = config.borrow_mut();
        if input.index >= config.0.servers.len() {
            return Err(crate::json_command::CommandError::new(
                "index_out_of_range",
                "server index does not exist",
            ));
        }
        config.0.servers.remove(input.index);
        config
            .save()
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        Ok(EmptyOutput {})
    });
    registry.register_typed::<SettingsShowDefinition>(|_, _| {
        let settings = &config.borrow().0.settings;
        Ok(SettingsShowOutput {
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
            ],
        })
    });
    registry.register_typed::<SettingsSetDefinition>(|input, _| {
        let mut config = config.borrow_mut();
        match input.key.as_str() {
            "controls.mouse_sensitivity" => {
                config.0.settings.controls.mouse_sensitivity = input.value
            }
            "camera.move_speed" => config.0.settings.camera.move_speed = input.value,
            "camera.voxel_raycast_distance" => {
                config.0.settings.camera.voxel_raycast_distance = input.value
            }
            "world.joinable_world_radius" => {
                config.0.settings.world.joinable_world_radius = input.value
            }
            _ => {
                return Err(crate::json_command::CommandError::new(
                    "invalid_arguments",
                    "unknown setting key or value",
                ));
            }
        }
        config.0.settings.normalize();
        config
            .save()
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        Ok(EmptyOutput {})
    });
    registry.register_typed::<BindingsListDefinition>(|_, _| {
        Ok(BindingsListOutput {
            bindings: config
                .borrow()
                .0
                .settings
                .key_bindings
                .iter()
                .map(binding_display)
                .collect(),
            supported_keys: supported_keys(),
            supported_actions: supported_actions(),
        })
    });
    registry.register_typed::<BindingsBindDefinition>(|input, _| {
        let binding = ClientKeyBindingConfig::try_from(input)?;
        let mut config = config.borrow_mut();
        match config
            .0
            .settings
            .key_bindings
            .iter_mut()
            .find(|existing| existing.key == binding.key)
        {
            Some(existing) => {
                if binding
                    .actions
                    .iter()
                    .all(|action| existing.actions.contains(action))
                {
                    return Err(crate::json_command::CommandError::new(
                        "binding_exists",
                        "binding already exists",
                    ));
                }
                let additions = binding
                    .actions
                    .into_iter()
                    .filter(|action| !existing.actions.contains(action))
                    .collect::<Vec<_>>();
                existing.actions.extend(additions);
            }
            None => config.0.settings.key_bindings.push(binding),
        }
        config
            .save()
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        Ok(EmptyOutput {})
    });
    registry.register_typed::<BindingsUnbindDefinition>(|input, _| {
        let binding = ClientKeyBindingConfig::try_from(input)?;
        let mut config = config.borrow_mut();
        let Some(index) = config
            .0
            .settings
            .key_bindings
            .iter()
            .position(|existing| existing.key == binding.key)
        else {
            return Err(crate::json_command::CommandError::new(
                "binding_not_found",
                "binding does not exist",
            ));
        };
        let existing = &mut config.0.settings.key_bindings[index];
        let before = existing.actions.len();
        existing
            .actions
            .retain(|action| !binding.actions.contains(action));
        if before == existing.actions.len() {
            return Err(crate::json_command::CommandError::new(
                "binding_not_found",
                "binding does not exist",
            ));
        }
        if existing.actions.is_empty() {
            config.0.settings.key_bindings.remove(index);
        }
        config
            .save()
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        Ok(EmptyOutput {})
    });
    registry.register_typed::<BindingsReplaceDefinition>(|input, _| {
        let old_binding = ClientKeyBindingConfig::try_from(input.old_binding)?;
        let replacement = ClientKeyBindingConfig::try_from(input.binding)?;
        let mut config = config.borrow_mut();
        let mut updated_config = config.0.clone();
        replace_binding(
            &mut updated_config.settings.key_bindings,
            old_binding,
            replacement,
        )?;
        roundo_user_config::save_config(CLIENT_CONFIG_FILE, &updated_config)
            .map_err(|error| crate::json_command::CommandError::new("config_save_failed", error))?;
        config.0 = updated_config;
        Ok(BindingMutationOutput {})
    });
    registry.register_typed::<CommandSchemaDefinition>(|input, _| {
        client_command_schema(&input.command).map(|schema| {
            CommandSchemaOutput(schema.as_object().cloned().expect("schema is an object"))
        })
    });
    registry.register_typed::<CommandHelpDefinition>(|input, _| match input.command {
        None => Ok(CommandHelpOutput::Commands {
            commands: visible_commands(dev_level.borrow().0),
        }),
        Some(command) => match command_dev_level(&command) {
            Some(required_level) => Ok(CommandHelpOutput::Command {
                command,
                dev_level: required_level,
            }),
            None => Err(crate::json_command::CommandError::new(
                "unknown_command",
                "unknown command",
            )),
        },
    });
    registry.register_typed::<DevDefinition>(|input, _| {
        let mut dev_level = dev_level.borrow_mut();
        if let Some(level) = input.level {
            if level > 3 {
                return Err(crate::json_command::CommandError::new(
                    "invalid_arguments",
                    "level must be 0 through 3",
                ));
            }
            dev_level.0 = level;
        }
        Ok(DevOutput { level: dev_level.0 })
    });
    registry.register_typed::<HudShowDefinition>(|_, _| {
        let target = hud
            .target
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| {
                crate::json_command::CommandError::new("internal_command_error", error.to_string())
            })?;
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
    });
    registry.register_typed::<DiagnosticsPositionDefinition>(|_, _| {
        let transform = controller
            .camera()
            .and_then(|entity| cameras.get(entity).ok())
            .ok_or_else(|| {
                crate::json_command::CommandError::new(
                    "camera_unavailable",
                    "player camera is unavailable",
                )
            })?;
        Ok(PositionOutput {
            x: transform.translation.x,
            y: transform.translation.y,
            z: transform.translation.z,
        })
    });
    registry.register_typed::<AppQuitDefinition>(|_, _| {
        Ok(AcceptedOutput {
            status: "accepted".into(),
        })
    });
    registry.register_typed::<OpenExternalUrlDefinition>(|input, _| {
        let url = validate_external_url(&input.url)?;
        open_external_url(url).map_err(|error| {
            crate::json_command::CommandError::new("external_url_open_failed", error.to_string())
        })?;
        Ok(AcceptedOutput {
            status: "accepted".into(),
        })
    });
    registry.register_typed::<UiBackDefinition>(|_, _| {
        let source = ui_source.ok_or_else(|| {
            crate::json_command::CommandError::new(
                "stale_ui_instance",
                "ui.back requires a live WebView source",
            )
        })?;
        let mut webui = webui.borrow_mut();
        let manager = webui.as_deref_mut().ok_or_else(|| {
            crate::json_command::CommandError::new("stale_ui_instance", "Web UI is unavailable")
        })?;
        let mut navigation = navigation.borrow_mut();
        let navigation = navigation.as_deref_mut().ok_or_else(|| {
            crate::json_command::CommandError::new(
                "ui_navigation_failed",
                "Web UI navigation is unavailable",
            )
        })?;
        navigation
            .back(manager, source)
            .map_err(map_ui_lifecycle_error)?;
        Ok(EmptyOutput {})
    });
    registry.register_typed::<UiOpenDefinition>(|input, _| {
        let mut webui = webui.borrow_mut();
        let state = webui.as_deref_mut().ok_or_else(|| {
            crate::json_command::CommandError::new("invalid_ui_resource", "Web UI is unavailable")
        })?;
        let target = match (input.slot, input.resource) {
            (Some(slot), None) => state.resolve_slot_path(&slot, input.path.as_deref()),
            (None, Some(resource)) => state.resolve_resource_path(&resource, input.path.as_deref()),
            _ => Err(roundo_webui::UiRegistryError::InvalidReference(
                "ui.open requires exactly one slot or resource".into(),
            )),
        }
        .map_err(|error| {
            crate::json_command::CommandError::new("invalid_ui_resource", error.to_string())
        })?;
        let resource = target.resource().to_owned();
        log::debug!("Resolved Web UI navigation target `{resource}`");
        let mut navigation = navigation.borrow_mut();
        let navigation = navigation.as_deref_mut().ok_or_else(|| {
            crate::json_command::CommandError::new(
                "ui_navigation_failed",
                "Web UI navigation is unavailable",
            )
        })?;
        let source = ui_source
            .map(roundo_webui::UiCommandSource::WebView)
            .unwrap_or(roundo_webui::UiCommandSource::Host);
        navigation.open(state, source, target).map_err(|error| {
            log::error!("Web UI navigation to `{resource}` failed: {error}");
            map_ui_lifecycle_error(error)
        })?;
        log::info!("Web UI navigated to `{resource}`");
        Ok(UiOpenOutput { resource })
    });
    assert_catalog_matches_registry(&registry);
    registry.dispatch_value(request, &mut ())
}

fn update_hud_cache(
    time: Res<Time>,
    controller: Res<ClientPlayerController>,
    cameras: Query<&Transform>,
    mut hud: ResMut<HudCache>,
) {
    hud.record_frame(time.delta_secs());
    hud.position = controller
        .camera()
        .and_then(|entity| cameras.get(entity).ok())
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

fn submit_terminal_request(
    pipe: &ClientCommandPipe,
    responses: &mut TerminalResponses,
    request: serde_json::Value,
) -> Option<serde_json::Value> {
    match pipe.io().submit(request) {
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
    use super::{CLIENT_COMMANDS, command_dev_level, validate_external_url, visible_commands};
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
        assert!(!visible_commands(0).contains(&"hud.show"));
        assert!(visible_commands(1).contains(&"hud.show"));
        assert_eq!(command_dev_level("hud.show"), Some(1));
        assert_eq!(command_dev_level("missing"), None);
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
        let status = super::client_command_schema("server.status").unwrap();
        assert!(
            status["success"]["properties"]["data"]["properties"]
                .get("status")
                .is_some()
        );
        assert!(
            status["success"]["properties"]["data"]["properties"]["status"]
                .get("$ref")
                .is_some()
        );

        for command in ["server.list", "settings.show", "bindings.list"] {
            let schema = super::client_command_schema(command).unwrap();
            assert_eq!(schema["success"]["properties"]["data"]["type"], "object");
        }
        let list = super::client_command_schema("server.list").unwrap();
        assert!(
            list["success"]["properties"]["data"]["properties"]
                .get("servers")
                .is_some()
        );
        let settings = super::client_command_schema("settings.show").unwrap();
        assert!(
            settings["success"]["properties"]["data"]["properties"]
                .get("settings")
                .is_some()
        );
        let bindings = super::client_command_schema("bindings.list").unwrap();
        assert!(
            bindings["success"]["properties"]["data"]["properties"]
                .get("supported_keys")
                .is_some()
        );
        let meta = super::client_command_schema("command.help").unwrap();
        assert!(meta["success"]["properties"]["data"].get("anyOf").is_some());
        let schema_meta = super::client_command_schema("command.schema").unwrap();
        assert_eq!(
            schema_meta["success"]["properties"]["data"]["type"],
            "object"
        );
    }

    #[test]
    fn binding_input_rejects_empty_duplicate_and_unknown_values() {
        use std::convert::TryFrom;
        let valid = super::BindingArguments {
            key: "key_w".into(),
            actions: vec!["move_forward".into()],
        };
        assert!(roundo_user_config::ClientKeyBindingConfig::try_from(valid).is_ok());
        for arguments in [
            super::BindingArguments {
                key: "key_w".into(),
                actions: vec![],
            },
            super::BindingArguments {
                key: "key_w".into(),
                actions: vec!["move_forward".into(), "move_forward".into()],
            },
            super::BindingArguments {
                key: "unknown".into(),
                actions: vec!["move_forward".into()],
            },
        ] {
            assert!(roundo_user_config::ClientKeyBindingConfig::try_from(arguments).is_err());
        }
    }

    #[test]
    fn bindings_list_display_model_adapts_config_enums() {
        let keys = super::supported_keys();
        assert_eq!(keys.len(), 52);
        assert_eq!(keys[0].key, "escape");
        assert_eq!(keys[0].display_name, "Escape");
        assert_eq!(keys.last().unwrap().key, "arrow_right");
        assert!(keys.iter().all(|key| !key.display_name.is_empty()));

        let actions = super::supported_actions();
        assert_eq!(actions.len(), 6);
        assert_eq!(actions[4].action, "move_forward");
        assert_eq!(actions[4].display_name, "Move Forward");

        let binding = roundo_user_config::ClientKeyBindingConfig::new(
            roundo_user_config::ClientKeyCode::KeyW,
            roundo_user_config::ClientMovementAction::MoveForward,
        );
        let display = super::binding_display(&binding);
        assert_eq!(display.key, "key_w");
        assert_eq!(display.actions, ["move_forward"]);
    }

    #[test]
    fn bindings_replace_input_is_strict_and_reuses_binding_validation() {
        let valid = serde_json::json!({
            "old_binding": {"key": "key_w", "actions": ["move_forward"]},
            "binding": {"key": "key_e", "actions": ["move_forward"]},
        });
        let parsed: super::BindingsReplaceArguments = serde_json::from_value(valid).unwrap();
        assert!(roundo_user_config::ClientKeyBindingConfig::try_from(parsed.old_binding).is_ok());
        assert!(roundo_user_config::ClientKeyBindingConfig::try_from(parsed.binding).is_ok());

        assert!(
            serde_json::from_value::<super::BindingsReplaceArguments>(serde_json::json!({
                "old_binding": {"key": "key_w", "actions": ["move_forward"]},
                "binding": {"key": "key_e", "actions": ["move_forward"]},
                "unexpected": true,
            }))
            .is_err()
        );
        let invalid: super::BindingsReplaceArguments = serde_json::from_value(serde_json::json!({
            "old_binding": {"key": "key_w", "actions": []},
            "binding": {"key": "key_e", "actions": ["move_forward", "move_forward"]},
        }))
        .unwrap();
        assert!(roundo_user_config::ClientKeyBindingConfig::try_from(invalid.old_binding).is_err());
        assert!(roundo_user_config::ClientKeyBindingConfig::try_from(invalid.binding).is_err());

        let schema = crate::json_command::command_schema::<super::BindingsReplaceDefinition>();
        assert_eq!(schema["command"], "bindings.replace");
        assert_eq!(schema["input"]["properties"]["arguments"]["type"], "object");
        assert_eq!(
            schema["input"]["properties"]["arguments"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn bindings_replace_is_atomic_in_memory_and_rejects_missing_or_duplicate_keys() {
        use roundo_user_config::{ClientKeyBindingConfig, ClientKeyCode, ClientMovementAction};

        let old =
            ClientKeyBindingConfig::new(ClientKeyCode::KeyW, ClientMovementAction::MoveForward);
        let replacement =
            ClientKeyBindingConfig::new(ClientKeyCode::KeyE, ClientMovementAction::MoveForward);
        let mut bindings = vec![old.clone()];
        super::replace_binding(&mut bindings, old.clone(), replacement.clone()).unwrap();
        assert_eq!(bindings, vec![replacement.clone()]);

        let before_missing = bindings.clone();
        let missing =
            ClientKeyBindingConfig::new(ClientKeyCode::KeyQ, ClientMovementAction::MoveLeft);
        let error = super::replace_binding(&mut bindings, missing, old.clone()).unwrap_err();
        assert_eq!(error.code, "binding_not_found");
        assert_eq!(bindings, before_missing);

        let existing =
            ClientKeyBindingConfig::new(ClientKeyCode::KeyA, ClientMovementAction::MoveLeft);
        bindings.push(existing.clone());
        let before_duplicate = bindings.clone();
        let error = super::replace_binding(&mut bindings, replacement, existing).unwrap_err();
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
        let _occupied = pipe.io().submit(serde_json::json!({})).unwrap();
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
