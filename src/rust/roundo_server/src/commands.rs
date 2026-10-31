//! Server-console projection into the shared typed JSON command dispatcher.
//!
//! Terminal text is tokenized and projected into versioned JSON envelopes before
//! execution against the Bevy main world. Unix aliases never create independent
//! command identities or schemas.

use bevy::{
    app::AppExit,
    prelude::{App, MessageWriter, Res, ResMut, Resource, Update},
};
use roundo_cli::{
    TerminalInput,
    json_command::{
        CommandDefinition, CommandError, CommandRegistry, UnixCommandRegistry, command_schema,
        tokenize_unix_line,
    },
};
use roundo_presence::{PlayerId, PresenceAdmin, SceneId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

struct CatalogEntry {
    name: &'static str,
    dev_level: u8,
    schema: fn() -> serde_json::Value,
}

impl CatalogEntry {
    const fn of<D: CommandDefinition>() -> Self {
        Self {
            name: D::NAME,
            dev_level: D::DEV_LEVEL,
            schema: command_schema::<D>,
        }
    }
}

// Discovery metadata in deterministic display order. Dev level controls catalog
// visibility only; it is not an authorization boundary for direct dispatch.
const SERVER_COMMANDS: &[CatalogEntry] = &[
    CatalogEntry::of::<AppQuitDefinition>(),
    CatalogEntry::of::<CommandHelpDefinition>(),
    CatalogEntry::of::<PlayersListDefinition>(),
    CatalogEntry::of::<PlayerPositionDefinition>(),
    CatalogEntry::of::<PlayerTeleportDefinition>(),
    CatalogEntry::of::<DevDefinition>(),
    CatalogEntry::of::<CommandSchemaDefinition>(),
    CatalogEntry::of::<BlockItemPermissionCommand>(),
];

/// Console-local command discovery level, restricted by the `dev` handler to 0..=3.
#[derive(Resource, Default)]
struct ServerDevLevel(u8);

/// Canonical Unix paths and adapter-only aliases for server commands.
#[derive(Resource)]
struct ServerUnixAdapter {
    registry: UnixCommandRegistry,
}
impl Default for ServerUnixAdapter {
    fn default() -> Self {
        let mut registry = UnixCommandRegistry::default();
        registry.register::<UnixPlayersList>();
        registry.register::<UnixPlayerPosition>();
        registry.register::<UnixPlayerTeleport>();
        registry.register::<UnixAppQuit>();
        registry.register::<UnixCommandHelp>();
        registry.register::<UnixCommandSchema>();
        registry.register::<UnixDev>();
        registry.register::<BlockItemPermissionCommand>();

        // These spellings are adapter-only aliases for the canonical JSON
        // commands above; they do not create additional definitions or schema.
        registry.register_alias::<UnixPlayersList>(&["players"]);
        registry.register_alias::<UnixPlayersList>(&["list"]);
        registry.register_alias::<UnixPlayerPosition>(&["position"]);
        registry.register_alias::<UnixPlayerPosition>(&["pos"]);
        registry.register_alias::<UnixPlayerTeleport>(&["teleport"]);
        registry.register_alias::<UnixPlayerTeleport>(&["tp"]);
        registry.register_alias::<UnixAppQuit>(&["quit"]);
        registry.register_alias::<UnixAppQuit>(&["exit"]);
        registry.register_alias::<UnixCommandHelp>(&["help"]);
        registry.register_alias::<UnixCommandHelp>(&["?"]);
        Self { registry }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlayerIdArguments {
    player_id: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TeleportArguments {
    player_id: u64,
    translation: [f32; 3],
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

#[derive(Serialize, JsonSchema)]
struct AcceptedOutput {
    status: String,
}
#[derive(Serialize, JsonSchema)]
struct PlayerDisplay {
    player_id: u64,
    scene: String,
    position: [f32; 3],
}
#[derive(Serialize, JsonSchema)]
struct PlayersListOutput {
    players: Vec<PlayerDisplay>,
}
#[derive(Serialize, JsonSchema)]
struct PlayerPositionOutput {
    player_id: u64,
    scene: String,
    position: [f32; 3],
}
#[derive(Serialize, JsonSchema)]
struct PlayerTeleportOutput {
    player_id: u64,
    position: [f32; 3],
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

// Keeps each canonical JSON name, input, output, and discovery level co-located.
macro_rules! definition {
    ($name:ident, $input:ty, $output:ty, $command:literal, $level:literal) => {
        struct $name;
        impl CommandDefinition for $name {
            type Input = $input;
            type Output = $output;
            const NAME: &'static str = $command;
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
    PlayersListDefinition,
    EmptyArguments,
    PlayersListOutput,
    "players.list",
    0
);
definition!(
    PlayerPositionDefinition,
    PlayerIdArguments,
    PlayerPositionOutput,
    "player.position",
    0
);
definition!(
    PlayerTeleportDefinition,
    TeleportArguments,
    PlayerTeleportOutput,
    "player.teleport",
    0
);
definition!(
    CommandHelpDefinition,
    CommandHelpArguments,
    CommandHelpOutput,
    "command.help",
    0
);
definition!(
    CommandSchemaDefinition,
    CommandSchemaArguments,
    CommandSchemaOutput,
    "command.schema",
    1
);
definition!(DevDefinition, DevArguments, DevOutput, "dev", 0);

macro_rules! unix_empty_command {
    ($name:ident, $definition:ty, $path:literal) => {
        #[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
        #[serde(deny_unknown_fields)]
        #[unix(path = $path)]
        struct $name {}
        impl CommandDefinition for $name {
            type Input = Self;
            type Output = <$definition as CommandDefinition>::Output;
            const NAME: &'static str = <$definition as CommandDefinition>::NAME;
            const DEV_LEVEL: u8 = <$definition as CommandDefinition>::DEV_LEVEL;
        }
    };
}

unix_empty_command!(UnixPlayersList, PlayersListDefinition, "players list");
unix_empty_command!(UnixAppQuit, AppQuitDefinition, "app quit");

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "player position")]
struct UnixPlayerPosition {
    player_id: u64,
}
impl CommandDefinition for UnixPlayerPosition {
    type Input = Self;
    type Output = <PlayerPositionDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <PlayerPositionDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <PlayerPositionDefinition as CommandDefinition>::DEV_LEVEL;
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnixPlayerTeleport {
    player_id: u64,
    x: f32,
    y: f32,
    z: f32,
}
impl CommandDefinition for UnixPlayerTeleport {
    type Input = Self;
    type Output = <PlayerTeleportDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <PlayerTeleportDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <PlayerTeleportDefinition as CommandDefinition>::DEV_LEVEL;
}

impl roundo_cli::UnixCommand for UnixPlayerTeleport {
    const UNIX_PATH: &'static [&'static str] = &["player", "teleport"];

    fn parse_unix(
        arguments: &[String],
    ) -> Result<serde_json::Value, roundo_cli::UnixCommandParseError> {
        if arguments.len() != 4 {
            return Err(roundo_cli::UnixCommandParseError::new(
                "usage: player teleport <player_id> <x> <y> <z>",
            ));
        }
        let player_id = arguments[0]
            .parse::<u64>()
            .map_err(|_| roundo_cli::UnixCommandParseError::new("invalid argument 1"))?;
        let translation = [
            parse_finite_coordinate(&arguments[1], 2)?,
            parse_finite_coordinate(&arguments[2], 3)?,
            parse_finite_coordinate(&arguments[3], 4)?,
        ];
        Ok(serde_json::json!({"player_id": player_id, "translation": translation}))
    }

    fn usage() -> String {
        "player teleport <player_id> <x> <y> <z>".into()
    }
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "command help")]
struct UnixCommandHelp {
    command: Option<String>,
}
impl CommandDefinition for UnixCommandHelp {
    type Input = Self;
    type Output = <CommandHelpDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <CommandHelpDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <CommandHelpDefinition as CommandDefinition>::DEV_LEVEL;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "command schema")]
struct UnixCommandSchema {
    command: String,
}
impl CommandDefinition for UnixCommandSchema {
    type Input = Self;
    type Output = <CommandSchemaDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <CommandSchemaDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <CommandSchemaDefinition as CommandDefinition>::DEV_LEVEL;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "dev")]
struct UnixDev {
    level: Option<u8>,
}
impl CommandDefinition for UnixDev {
    type Input = Self;
    type Output = <DevDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <DevDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <DevDefinition as CommandDefinition>::DEV_LEVEL;
}

/// Omit reach to remove that permission source; supply reach to compose it again.
#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "block-item permission")]
struct BlockItemPermissionCommand {
    method: String,
    reach: Option<f32>,
}

#[derive(Serialize, JsonSchema)]
struct BlockItemPermissionOutput {
    instance_id: String,
    method: String,
    placement_reach: Option<f32>,
    destruction_reach: Option<f32>,
}

impl CommandDefinition for BlockItemPermissionCommand {
    type Input = Self;
    type Output = BlockItemPermissionOutput;
    const NAME: &'static str = "block_item.permission";
    const DEV_LEVEL: u8 = 1;
}

/// Installs the server-owned command catalog over the generic CLI adapter.
pub(crate) struct ServerCommandsPlugin;

impl bevy::prelude::Plugin for ServerCommandsPlugin {
    fn build(&self, app: &mut App) {
        println!("[cli] Roundo server console ready; type 'help' for available commands");
        app.init_resource::<ServerDevLevel>();
        app.init_resource::<ServerUnixAdapter>();
        app.add_systems(Update, process_commands);
    }
}

/// Unix text is only terminal input projection. All command execution remains
/// in the source-neutral typed JSON registry below.
fn process_commands(
    input: Res<TerminalInput>,
    adapter: Res<ServerUnixAdapter>,
    mut players: PresenceAdmin,
    mut dev_level: ResMut<ServerDevLevel>,
    mut app_exit: MessageWriter<AppExit>,
    mut block_item: roundo_ecs_entry::BlockItemEdits,
) {
    for line in input.drain() {
        let Some(request) = project_terminal_line(&adapter.registry, &line) else {
            continue;
        };
        let response =
            dispatch_typed_server_command(request, &mut players, &mut dev_level, &mut block_item);
        let quit_accepted = response["command"] == "app.quit" && response["ok"] == true;
        print_response(response);
        if quit_accepted {
            let _exit_message = app_exit.write(AppExit::Success);
        }
    }
}

/// Projects one terminal line into a typed envelope or structured error.
///
/// Empty/token-only whitespace lines return `None`; tokenization and argument
/// failures return an error envelope rather than bypassing typed dispatch.
fn project_terminal_line(registry: &UnixCommandRegistry, line: &str) -> Option<serde_json::Value> {
    let tokens = match tokenize_unix_line(line) {
        Ok(tokens) if tokens.is_empty() => return None,
        Ok(tokens) => tokens,
        Err(error) => return Some(terminal_error("", "invalid_command_envelope", error)),
    };
    Some(match registry.project(&tokens) {
        Ok((command, arguments)) => serde_json::json!({
            "version": roundo_cli::json_command::COMMAND_VERSION,
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

/// Parses one Unix teleport coordinate and rejects non-finite `f32` values.
fn parse_finite_coordinate(
    value: &str,
    argument_number: usize,
) -> Result<f32, roundo_cli::UnixCommandParseError> {
    let coordinate = value.parse::<f32>().map_err(|_| {
        roundo_cli::UnixCommandParseError::new(format!("invalid argument {argument_number}"))
    })?;
    if coordinate.is_finite() {
        Ok(coordinate)
    } else {
        Err(roundo_cli::UnixCommandParseError::new(format!(
            "argument {argument_number} must be finite"
        )))
    }
}

/// Builds the source-neutral registry over short-lived main-world borrows and dispatches once.
///
/// Player lists are sorted by identity before serialization. Teleport mutation
/// occurs only after complete envelope and typed-input validation.
fn dispatch_typed_server_command(
    request: serde_json::Value,
    players: &mut PresenceAdmin,
    dev_level: &mut ServerDevLevel,
    block_item: &mut roundo_ecs_entry::BlockItemEdits,
) -> serde_json::Value {
    // This short-lived main-world context is the server's Bevy seam. The
    // registry remains source-neutral; only registered handlers borrow ECS.
    let players = RefCell::new(players);
    let dev_level = RefCell::new(dev_level);
    let block_item = RefCell::new(block_item);
    let mut registry = CommandRegistry::default();
    registry.register_typed::<PlayersListDefinition>(|_, _| {
        Ok(PlayersListOutput {
            players: players
                .borrow_mut()
                .players()
                .into_iter()
                .map(|player| PlayerDisplay {
                    player_id: player.player_id.0,
                    scene: scene_name(player.scene_id),
                    position: player.translation,
                })
                .collect(),
        })
    });
    registry.register_typed::<PlayerPositionDefinition>(|input, _| {
        let player_id = PlayerId(input.player_id);
        let player = players.borrow_mut().player(player_id).ok_or_else(|| {
            CommandError::new(
                "player_not_found",
                format!("player {} was not found", input.player_id),
            )
        })?;
        Ok(PlayerPositionOutput {
            player_id: player.player_id.0,
            scene: scene_name(player.scene_id),
            position: player.translation,
        })
    });
    registry.register_typed::<PlayerTeleportDefinition>(|input, _| {
        let position = players
            .borrow_mut()
            .teleport(PlayerId(input.player_id), input.translation)
            .ok_or_else(|| {
                CommandError::new(
                    "player_not_found_or_invalid_position",
                    format!(
                        "player {} was not found or the requested position was non-finite",
                        input.player_id
                    ),
                )
            })?;
        Ok(PlayerTeleportOutput {
            player_id: input.player_id,
            position,
        })
    });
    registry.register_typed::<AppQuitDefinition>(|_, _| {
        Ok(AcceptedOutput {
            status: "accepted".into(),
        })
    });
    registry.register_typed::<CommandHelpDefinition>(|input, _| match input.command {
        None => Ok(CommandHelpOutput::Commands {
            commands: visible_commands(dev_level.borrow().0),
        }),
        Some(command) => command_dev_level(&command)
            .map(|dev_level| CommandHelpOutput::Command { command, dev_level })
            .ok_or_else(|| CommandError::new("unknown_command", "unknown command")),
    });
    registry.register_typed::<CommandSchemaDefinition>(|input, _| {
        server_command_schema(&input.command).map(|schema| {
            CommandSchemaOutput(schema.as_object().cloned().expect("schema is an object"))
        })
    });
    registry.register_typed::<DevDefinition>(|input, _| {
        if let Some(level) = input.level {
            if level > 3 {
                return Err(CommandError::new(
                    "invalid_arguments",
                    "level must be 0 through 3",
                ));
            }
            dev_level.borrow_mut().0 = level;
        }
        Ok(DevOutput {
            level: dev_level.borrow().0,
        })
    });
    registry.register_typed::<BlockItemPermissionCommand>(|input, _| {
        let method = match input.method.as_str() {
            "place" => roundo_contracts::ItemUseMethod::PlaceBlock,
            "destroy" => roundo_contracts::ItemUseMethod::DestroyBlock,
            _ => {
                return Err(CommandError::new(
                    "invalid_arguments",
                    "method must be place or destroy",
                ));
            }
        };
        let item = block_item
            .borrow_mut()
            .set_permission(method, input.reach)
            .map_err(|error| CommandError::new("item_data_rejected", error))?;
        Ok(BlockItemPermissionOutput {
            instance_id: item.item.as_u128().to_string(),
            method: input.method,
            placement_reach: item.placement_reach,
            destruction_reach: item.destruction_reach,
        })
    });
    registry.dispatch_value(request, &mut ())
}

/// Generates the requested command's schema from its canonical typed definition.
fn server_command_schema(command: &str) -> Result<serde_json::Value, CommandError> {
    if command.is_empty() {
        return Err(CommandError::new(
            "invalid_arguments",
            "command must be a non-empty string",
        ));
    }
    SERVER_COMMANDS
        .iter()
        .find(|entry| entry.name == command)
        .map(|entry| (entry.schema)())
        .ok_or_else(|| CommandError::new("unknown_command", "schema is unavailable"))
}

/// Returns catalog names whose discovery level does not exceed `level`.
fn visible_commands(level: u8) -> Vec<&'static str> {
    SERVER_COMMANDS
        .iter()
        .filter_map(|entry| (entry.dev_level <= level).then_some(entry.name))
        .collect()
}

/// Looks up discovery metadata by canonical JSON command name.
fn command_dev_level(name: &str) -> Option<u8> {
    SERVER_COMMANDS
        .iter()
        .find_map(|entry| (entry.name == name).then_some(entry.dev_level))
}

/// Constructs the same versioned error shape returned by typed dispatch.
fn terminal_error(command: &str, code: &str, message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({
        "version": roundo_cli::json_command::COMMAND_VERSION,
        "command": command,
        "ok": false,
        "error": {"code": code, "message": message.to_string()},
    })
}

fn print_response(response: serde_json::Value) {
    println!("{response}");
}

/// Formats a scene identity for human-facing console output.
fn scene_name(scene: SceneId) -> String {
    match scene {
        SceneId::S0 { room_id } => format!("S0/{room_id}"),
        SceneId::S1 => "S1".to_string(),
    }
}
