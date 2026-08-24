use crate::{
    TerminalInput,
    json_command::{
        ClientCommandDefinition, CommandError, CommandRegistry, UnixCommandRegistry,
        command_schema, tokenize_unix_line,
    },
};
use bevy::{
    app::AppExit,
    prelude::{App, MessageWriter, Query, Res, ResMut, Resource, Transform, Update, Vec3, With},
};
use roundo_presence::{Player, PlayerScene, SceneId, ServerPlayer, ServerSceneWorlds};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

const SERVER_COMMANDS: &[(&str, u8)] = &[
    ("app.quit", 0),
    ("command.help", 0),
    ("players.list", 0),
    ("player.position", 0),
    ("player.teleport", 0),
    ("dev", 0),
    ("command.schema", 1),
];

#[derive(Resource, Default)]
struct ServerDevLevel(u8);

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

macro_rules! definition {
    ($name:ident, $input:ty, $output:ty, $command:literal, $level:literal) => {
        struct $name;
        impl ClientCommandDefinition for $name {
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

unix_empty_command!(
    UnixPlayersList,
    "players.list",
    "players list",
    PlayersListOutput
);
unix_empty_command!(UnixAppQuit, "app.quit", "app quit", AcceptedOutput);

#[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "player position")]
struct UnixPlayerPosition {
    player_id: u64,
}
impl ClientCommandDefinition for UnixPlayerPosition {
    type Input = Self;
    type Output = PlayerPositionOutput;
    const NAME: &'static str = "player.position";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnixPlayerTeleport {
    player_id: u64,
    x: f32,
    y: f32,
    z: f32,
}
impl ClientCommandDefinition for UnixPlayerTeleport {
    type Input = Self;
    type Output = PlayerTeleportOutput;
    const NAME: &'static str = "player.teleport";
    const DEV_LEVEL: u8 = 0;
}

impl crate::UnixCommand for UnixPlayerTeleport {
    const UNIX_PATH: &'static [&'static str] = &["player", "teleport"];

    fn parse_unix(arguments: &[String]) -> Result<serde_json::Value, crate::UnixCommandParseError> {
        if arguments.len() != 4 {
            return Err(crate::UnixCommandParseError::new(
                "usage: player teleport <player_id> <x> <y> <z>",
            ));
        }
        let player_id = arguments[0]
            .parse::<u64>()
            .map_err(|_| crate::UnixCommandParseError::new("invalid argument 1"))?;
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

pub(super) fn configure(app: &mut App) {
    println!("[cli] Roundo server console ready; type 'help' for available commands");
    app.init_resource::<ServerDevLevel>();
    app.init_resource::<ServerUnixAdapter>();
    app.add_systems(Update, process_commands);
}

/// Unix text is only terminal input projection. All command execution remains
/// in the source-neutral typed JSON registry below.
fn process_commands(
    input: Res<TerminalInput>,
    adapter: Res<ServerUnixAdapter>,
    scenes: Res<ServerSceneWorlds>,
    mut players: Query<(&Player, &PlayerScene, &mut Transform), With<ServerPlayer>>,
    mut dev_level: ResMut<ServerDevLevel>,
    mut app_exit: MessageWriter<AppExit>,
) {
    for line in input.drain() {
        let Some(request) = project_terminal_line(&adapter.registry, &line) else {
            continue;
        };
        let response =
            dispatch_typed_server_command(request, &scenes, &mut players, &mut dev_level);
        let quit_accepted = response["command"] == "app.quit" && response["ok"] == true;
        print_response(response);
        if quit_accepted {
            app_exit.write(AppExit::Success);
        }
    }
}

fn project_terminal_line(registry: &UnixCommandRegistry, line: &str) -> Option<serde_json::Value> {
    let tokens = match tokenize_unix_line(line) {
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

fn parse_finite_coordinate(
    value: &str,
    argument_number: usize,
) -> Result<f32, crate::UnixCommandParseError> {
    let coordinate = value.parse::<f32>().map_err(|_| {
        crate::UnixCommandParseError::new(format!("invalid argument {argument_number}"))
    })?;
    if coordinate.is_finite() {
        Ok(coordinate)
    } else {
        Err(crate::UnixCommandParseError::new(format!(
            "argument {argument_number} must be finite"
        )))
    }
}

fn dispatch_typed_server_command(
    request: serde_json::Value,
    scenes: &ServerSceneWorlds,
    players: &mut Query<(&Player, &PlayerScene, &mut Transform), With<ServerPlayer>>,
    dev_level: &mut ServerDevLevel,
) -> serde_json::Value {
    // This short-lived main-world context is the server's Bevy seam. The
    // registry remains source-neutral; only registered handlers borrow ECS.
    let players = RefCell::new(players);
    let dev_level = RefCell::new(dev_level);
    let mut registry = CommandRegistry::default();
    registry.register_typed::<PlayersListDefinition>(|_, _| {
        let mut entries = players
            .borrow_mut()
            .iter_mut()
            .map(|(player, scene, transform)| (player.id.0, scene.scene_id, transform.translation))
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(player_id, _, _)| *player_id);
        Ok(PlayersListOutput {
            players: entries
                .into_iter()
                .map(|(player_id, scene, position)| PlayerDisplay {
                    player_id,
                    scene: scene_name(scene),
                    position: [position.x, position.y, position.z],
                })
                .collect(),
        })
    });
    registry.register_typed::<PlayerPositionDefinition>(|input, _| {
        let mut players = players.borrow_mut();
        let Some((_, scene, transform)) = players
            .iter_mut()
            .find(|(player, _, _)| player.id.0 == input.player_id)
        else {
            return Err(CommandError::new(
                "player_not_found",
                format!("player {} was not found", input.player_id),
            ));
        };
        let position = transform.translation;
        Ok(PlayerPositionOutput {
            player_id: input.player_id,
            scene: scene_name(scene.scene_id),
            position: [position.x, position.y, position.z],
        })
    });
    registry.register_typed::<PlayerTeleportDefinition>(|input, _| {
        let mut players = players.borrow_mut();
        let Some((_, scene, mut transform)) = players
            .iter_mut()
            .find(|(player, _, _)| player.id.0 == input.player_id)
        else {
            return Err(CommandError::new(
                "player_not_found",
                format!("player {} was not found", input.player_id),
            ));
        };
        let Some(space) = scenes.space(scene.scene_id) else {
            return Err(CommandError::new(
                "scene_unavailable",
                format!("scene {} is unavailable", scene_name(scene.scene_id)),
            ));
        };
        transform.translation = space.wrap(Vec3::from_array(input.translation));
        Ok(PlayerTeleportOutput {
            player_id: input.player_id,
            position: [
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            ],
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
    registry.dispatch_value(request, &mut ())
}

fn server_command_schema(command: &str) -> Result<serde_json::Value, CommandError> {
    macro_rules! schema {
        ($definition:ty) => {
            Ok(command_schema::<$definition>())
        };
    }
    match command {
        "app.quit" => schema!(AppQuitDefinition),
        "players.list" => schema!(PlayersListDefinition),
        "player.position" => schema!(PlayerPositionDefinition),
        "player.teleport" => schema!(PlayerTeleportDefinition),
        "command.help" => schema!(CommandHelpDefinition),
        "command.schema" => schema!(CommandSchemaDefinition),
        "dev" => schema!(DevDefinition),
        "" => Err(CommandError::new(
            "invalid_arguments",
            "command must be a non-empty string",
        )),
        _ => Err(CommandError::new(
            "unknown_command",
            "schema is unavailable",
        )),
    }
}

fn visible_commands(level: u8) -> Vec<&'static str> {
    SERVER_COMMANDS
        .iter()
        .filter_map(|(name, required)| (*required <= level).then_some(*name))
        .collect()
}

fn command_dev_level(name: &str) -> Option<u8> {
    SERVER_COMMANDS
        .iter()
        .find_map(|(known, level)| (*known == name).then_some(*level))
}

fn terminal_error(command: &str, code: &str, message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({
        "version": crate::json_command::COMMAND_VERSION,
        "command": command,
        "ok": false,
        "error": {"code": code, "message": message.to_string()},
    })
}

fn print_response(response: serde_json::Value) {
    println!("{response}");
}

fn scene_name(scene: SceneId) -> String {
    match scene {
        SceneId::S0 { room_id } => format!("S0/{room_id}"),
        SceneId::S1 => "S1".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::App;

    #[test]
    fn terminal_projects_canonical_paths_and_aliases_to_the_same_typed_envelopes() {
        let adapter = ServerUnixAdapter::default();
        for line in ["players list", "players", "list"] {
            assert_eq!(
                project_terminal_line(&adapter.registry, line).unwrap()["command"],
                "players.list"
            );
        }
        for line in ["player position 7", "position 7", "pos 7"] {
            assert_eq!(
                project_terminal_line(&adapter.registry, line).unwrap(),
                serde_json::json!({
                    "version": 1,
                    "command": "player.position",
                    "arguments": {"player_id": 7},
                })
            );
        }
        for line in [
            "player teleport 7 -1 2 3",
            "teleport 7 -1 2 3",
            "tp 7 -1 2 3",
        ] {
            assert_eq!(
                project_terminal_line(&adapter.registry, line).unwrap(),
                serde_json::json!({
                    "version": 1,
                    "command": "player.teleport",
                    "arguments": {"player_id": 7, "translation": [-1.0, 2.0, 3.0]},
                })
            );
        }
        for line in ["app quit", "quit", "exit"] {
            assert_eq!(
                project_terminal_line(&adapter.registry, line).unwrap()["command"],
                "app.quit"
            );
        }
        assert_eq!(
            project_terminal_line(&adapter.registry, "command help player.position").unwrap()["command"],
            "command.help"
        );
        assert_eq!(
            project_terminal_line(&adapter.registry, "command schema player.position").unwrap()["command"],
            "command.schema"
        );
        assert_eq!(
            project_terminal_line(&adapter.registry, "dev 1").unwrap()["command"],
            "dev"
        );
    }

    #[test]
    fn terminal_projection_errors_are_typed_json() {
        let adapter = ServerUnixAdapter::default();
        let unknown = project_terminal_line(&adapter.registry, "unknown command").unwrap();
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"]["code"], "unknown_command");

        let invalid = project_terminal_line(&adapter.registry, "tp 7 NaN 2 3").unwrap();
        assert_eq!(invalid["ok"], false);
        assert_eq!(invalid["error"]["code"], "invalid_arguments");
    }

    #[test]
    fn teleport_updates_the_authoritative_player_transform_through_the_registry() {
        let mut app = App::new();
        app.init_resource::<ServerSceneWorlds>()
            .insert_resource(TerminalInput::buffered(["tp 7 -1 2 3"]))
            .init_resource::<ServerDevLevel>()
            .init_resource::<ServerUnixAdapter>()
            .add_message::<AppExit>()
            .add_systems(Update, process_commands);
        let player = app
            .world_mut()
            .spawn((
                Player {
                    id: roundo_presence::PlayerId(7),
                },
                PlayerScene {
                    scene_id: SceneId::S1,
                },
                ServerPlayer,
                Transform::default(),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Transform>(player).unwrap().translation,
            Vec3::new(16_383.0, 2.0, 3.0)
        );
    }

    #[test]
    fn schemas_and_discovery_are_typed_definition_metadata() {
        let schema = server_command_schema("player.teleport").unwrap();
        assert_eq!(schema["command"], "player.teleport");
        assert!(
            schema["success"]["properties"]["data"]["properties"]
                .get("player_id")
                .is_some()
        );
        let list = server_command_schema("players.list").unwrap();
        assert!(
            list["success"]["properties"]["data"]["properties"]
                .get("players")
                .is_some()
        );
        let position = server_command_schema("player.position").unwrap();
        assert!(
            position["success"]["properties"]["data"]["properties"]
                .get("scene")
                .is_some()
        );
        assert_eq!(command_dev_level("player.position"), Some(0));
        assert!(!visible_commands(0).contains(&"command.schema"));
    }
}
