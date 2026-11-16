//! Unix text projection and one-shot terminal responses; no parallel command logic.

use super::*;

macro_rules! unix_empty_command {
    ($name:ident, $command:literal, $path:literal, $output:ty) => {
        #[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
        #[serde(deny_unknown_fields)]
        #[unix(path = $path)]
        pub(super) struct $name {}
        impl CommandDefinition for $name {
            type Input = Self;
            type Output = $output;
            const NAME: &'static str = $command;
            const DEV_LEVEL: u8 = 0;
        }
    };
}

// Scene-related projections delegate names, discovery levels and outputs to the
// canonical definitions; no parallel flow logic or metadata lives in this adapter.
macro_rules! unix_scene_command {
    ($name:ident, $definition:ty, $path:literal) => {
        #[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
        #[serde(deny_unknown_fields)]
        #[unix(path = $path)]
        pub(super) struct $name {}
        impl CommandDefinition for $name {
            type Input = Self;
            type Output = <$definition as CommandDefinition>::Output;
            const NAME: &'static str = <$definition as CommandDefinition>::NAME;
            const DEV_LEVEL: u8 = <$definition as CommandDefinition>::DEV_LEVEL;
        }
    };
}
unix_scene_command!(UnixScenePause, ScenePauseDefinition, "scene pause");
unix_scene_command!(UnixSceneResume, SceneResumeDefinition, "scene resume");

unix_empty_command!(UnixAppQuit, "app.quit", "app quit", AcceptedOutput);
unix_empty_command!(UnixUiBack, "ui.back", "ui back", EmptyOutput);
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

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "command help")]
pub(super) struct UnixCommandHelp {
    pub(super) command: Option<String>,
}
impl CommandDefinition for UnixCommandHelp {
    type Input = Self;
    type Output = CommandHelpOutput;
    const NAME: &'static str = "command.help";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "command schema")]
pub(super) struct UnixCommandSchema {
    pub(super) command: String,
}
impl CommandDefinition for UnixCommandSchema {
    type Input = Self;
    type Output = CommandSchemaOutput;
    const NAME: &'static str = "command.schema";
    const DEV_LEVEL: u8 = 1;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "dev")]
pub(super) struct UnixDev {
    pub(super) level: Option<u8>,
}
impl CommandDefinition for UnixDev {
    type Input = Self;
    type Output = DevOutput;
    const NAME: &'static str = "dev";
    const DEV_LEVEL: u8 = 0;
}

#[derive(Deserialize, Serialize, JsonSchema, roundo_cli::UnixCommand)]
#[serde(deny_unknown_fields)]
#[unix(path = "settings set")]
pub(super) struct UnixSettingsSet {
    pub(super) key: String,
    pub(super) value: f32,
}
impl CommandDefinition for UnixSettingsSet {
    type Input = Self;
    type Output = EmptyOutput;
    const NAME: &'static str = "settings.set";
    const DEV_LEVEL: u8 = 0;
}

fn project_binding_arguments(
    arguments: &[String],
    usage: &str,
) -> Result<serde_json::Value, UnixCommandParseError> {
    let [slot, key] = arguments else {
        return Err(UnixCommandParseError::new(format!("usage: {usage}")));
    };
    serde_json::to_value(BindingArguments {
        slot: slot.clone(),
        key: key.clone(),
    })
    .map_err(|error| UnixCommandParseError::new(error.to_string()))
}

pub(super) struct UnixBindingsBind;
impl CommandDefinition for UnixBindingsBind {
    type Input = <BindingsBindDefinition as CommandDefinition>::Input;
    type Output = <BindingsBindDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <BindingsBindDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <BindingsBindDefinition as CommandDefinition>::DEV_LEVEL;
}
impl UnixCommand for UnixBindingsBind {
    const UNIX_PATH: &'static [&'static str] = &["bindings", "bind"];
    fn parse_unix(arguments: &[String]) -> Result<serde_json::Value, UnixCommandParseError> {
        project_binding_arguments(arguments, &Self::usage())
    }
    fn usage() -> String {
        "bindings bind <slot> <key>".into()
    }
}

pub(super) struct UnixBindingsUnbind;
impl CommandDefinition for UnixBindingsUnbind {
    type Input = <BindingsUnbindDefinition as CommandDefinition>::Input;
    type Output = <BindingsUnbindDefinition as CommandDefinition>::Output;
    const NAME: &'static str = <BindingsUnbindDefinition as CommandDefinition>::NAME;
    const DEV_LEVEL: u8 = <BindingsUnbindDefinition as CommandDefinition>::DEV_LEVEL;
}
impl UnixCommand for UnixBindingsUnbind {
    const UNIX_PATH: &'static [&'static str] = &["bindings", "unbind"];
    fn parse_unix(arguments: &[String]) -> Result<serde_json::Value, UnixCommandParseError> {
        project_binding_arguments(arguments, &Self::usage())
    }
    fn usage() -> String {
        "bindings unbind <slot> <key>".into()
    }
}

#[derive(Resource)]
pub(super) struct ClientUnixAdapter {
    pub(super) registry: UnixCommandRegistry,
}
impl Default for ClientUnixAdapter {
    fn default() -> Self {
        let mut registry = UnixCommandRegistry::default();
        registry.register::<UnixAppQuit>();
        registry.register::<UnixUiBack>();
        registry.register::<UnixScenePause>();
        registry.register::<UnixSceneResume>();
        registry.register::<UnixCommandHelp>();
        registry.register::<UnixCommandSchema>();
        registry.register::<UnixDev>();
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
pub(super) struct TerminalResponses(pub(super) Vec<PendingTerminalResponse>);

pub(super) struct PendingTerminalResponse {
    pub(super) command: String,
    pub(super) call: RequestCall<serde_json::Value>,
}

pub(super) fn process_commands(
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
pub(super) fn project_terminal_line(
    registry: &UnixCommandRegistry,
    line: &str,
) -> Option<serde_json::Value> {
    let tokens = match roundo_cli::json_command::tokenize_unix_line(line) {
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

/// Attempts non-blocking host-authority admission to the shared JSON queue.
pub(super) fn submit_terminal_request(
    pipe: &ClientCommandPipe,
    responses: &mut TerminalResponses,
    request: serde_json::Value,
) -> Option<serde_json::Value> {
    let command = request
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match pipe.io().submit(request, roundo_ui::UiCommandSource::Host) {
        Ok(call) => {
            responses.0.push(PendingTerminalResponse { command, call });
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

/// Partitions ready, pending, and permanently disconnected response paths.
pub(super) fn collect_terminal_responses(
    responses: &mut TerminalResponses,
) -> Vec<serde_json::Value> {
    let mut pending = Vec::new();
    let mut completed = Vec::new();
    for response in std::mem::take(&mut responses.0) {
        match response.call.poll() {
            ResponsePoll::Ready(value) => completed.push(value),
            ResponsePoll::Pending => pending.push(response),
            ResponsePoll::Disconnected => completed.push(terminal_error(
                &response.command,
                "internal_command_error",
                "command response channel disconnected",
            )),
        }
    }
    responses.0 = pending;
    completed
}

pub(super) fn terminal_error(
    command: &str,
    code: &str,
    message: impl std::fmt::Display,
) -> serde_json::Value {
    serde_json::json!({
        "version": roundo_cli::json_command::COMMAND_VERSION,
        "command": command,
        "ok": false,
        "error": {"code": code, "message": message.to_string()},
    })
}

pub(super) fn print_terminal_response(response: serde_json::Value) {
    println!("{response}");
}
