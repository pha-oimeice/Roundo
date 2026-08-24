//! Versioned JSON command envelope and source-neutral dispatcher.
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::BTreeMap;

pub const COMMAND_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;

/// A command definition owns its typed input/output contract. Adapters only
/// project to this seam; schema generation never copies hand-written examples.
pub trait ClientCommandDefinition {
    type Input: DeserializeOwned + JsonSchema;
    type Output: Serialize + JsonSchema;
    const NAME: &'static str;
    const DEV_LEVEL: u8;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnixCommandParseError(String);
impl UnixCommandParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub fn is_unknown_command(&self) -> bool {
        self.0 == "unknown command"
    }
}
impl std::fmt::Display for UnixCommandParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for UnixCommandParseError {}

/// Opt-in Unix text projection. The command dispatcher itself never accepts
/// text, preserving one source-neutral JSON execution seam.
pub trait UnixCommand: ClientCommandDefinition {
    const UNIX_PATH: &'static [&'static str];
    fn parse_unix(arguments: &[String]) -> Result<Value, UnixCommandParseError>;
    fn usage() -> String;
}

/// Direct Unix-token index. The terminal adapter uses this deep module rather
/// than scanning every definition after each line.
#[derive(Default)]
pub struct UnixCommandRegistry {
    commands: BTreeMap<Vec<String>, UnixCommandEntry>,
}
struct UnixCommandEntry {
    command: &'static str,
    usage: fn() -> String,
    parse: fn(&[String]) -> Result<Value, UnixCommandParseError>,
}
impl UnixCommandRegistry {
    pub fn register<D: UnixCommand>(&mut self) {
        self.register_path::<D>(D::UNIX_PATH);
    }

    /// Registers an adapter-only spelling that projects to the same typed JSON
    /// command as `D`; aliases never create another command schema.
    pub fn register_alias<D: UnixCommand>(&mut self, path: &'static [&'static str]) {
        self.register_path::<D>(path);
    }

    fn register_path<D: UnixCommand>(&mut self, path: &[&str]) {
        let path = path
            .iter()
            .map(|token| token.to_ascii_lowercase())
            .collect();
        assert!(
            self.commands
                .insert(
                    path,
                    UnixCommandEntry {
                        command: D::NAME,
                        usage: D::usage,
                        parse: D::parse_unix
                    }
                )
                .is_none(),
            "duplicate Unix command path"
        );
    }
    pub fn project(
        &self,
        tokens: &[String],
    ) -> Result<(&'static str, Value), UnixCommandParseError> {
        let matching = (1..=tokens.len())
            .rev()
            .find_map(|length| {
                let path = tokens[..length]
                    .iter()
                    .map(|token| token.trim_start_matches('/').to_ascii_lowercase())
                    .collect::<Vec<_>>();
                self.commands.get(&path).map(|entry| (length, entry))
            })
            .ok_or_else(|| UnixCommandParseError::new("unknown command"))?;
        Ok((
            matching.1.command,
            (matching.1.parse)(&tokens[matching.0..])?,
        ))
    }
    pub fn usage(&self, tokens: &[String]) -> Option<String> {
        let path = tokens
            .iter()
            .map(|token| token.trim_start_matches('/').to_ascii_lowercase())
            .collect::<Vec<_>>();
        self.commands.get(&path).map(|entry| (entry.usage)())
    }
}

/// Splits a terminal line into Unix-style tokens without implementing shell
/// expansion, redirection, pipes, or command substitution. Quotes and a
/// backslash escape only affect token boundaries.
pub fn tokenize_unix_line(line: &str) -> Result<Vec<String>, UnixCommandParseError> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut token_started = false;

    for character in line.chars() {
        if escaped {
            token.push(character);
            token_started = true;
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            token_started = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                token.push(character);
            }
            token_started = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            token_started = true;
        } else if character.is_whitespace() {
            if token_started {
                tokens.push(std::mem::take(&mut token));
                token_started = false;
            }
        } else {
            token.push(character);
            token_started = true;
        }
    }
    if escaped {
        return Err(UnixCommandParseError::new("unterminated escape"));
    }
    if quote.is_some() {
        return Err(UnixCommandParseError::new("unterminated quote"));
    }
    if token_started {
        tokens.push(token);
    }
    Ok(tokens)
}

pub fn command_schema<D: ClientCommandDefinition>() -> Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "command": D::NAME,
        "input": {
            "type": "object",
            "required": ["version", "command", "arguments"],
            "properties": {
                "version": {"const": COMMAND_VERSION},
                "command": {"const": D::NAME},
                "arguments": schema_for!(D::Input)
            },
            "additionalProperties": false
        },
        "success": {
            "type": "object",
            "required": ["version", "command", "ok", "data"],
            "properties": {"version":{"const":COMMAND_VERSION}, "command":{"const":D::NAME}, "ok":{"const":true}, "data":schema_for!(D::Output)}
        },
        "error": {
            "type": "object",
            "required": ["version", "command", "ok", "error"],
            "properties": {"version":{"const":COMMAND_VERSION}, "command":{"const":D::NAME}, "ok":{"const":false}, "error":{"type":"object","required":["code","message"]}}
        }
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
    pub version: u32,
    pub command: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CommandResult {
    pub version: u32,
    pub command: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}
impl CommandError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

type Handler<'a, Context> = Box<dyn Fn(Value, &mut Context) -> Result<Value, CommandError> + 'a>;

/// Command definition registry. It deliberately has no UI/terminal knowledge;
/// adapters submit exactly the same envelope to it. `Context` is the Bevy-world
/// adapter seam: typed definitions remain source-neutral while handlers can
/// operate on the main-world state without pretending it is `Send + Sync`.
pub struct CommandRegistry<'a, Context = ()> {
    handlers: BTreeMap<String, Handler<'a, Context>>,
}
impl<'a, Context> Default for CommandRegistry<'a, Context> {
    fn default() -> Self {
        Self {
            handlers: BTreeMap::new(),
        }
    }
}
impl<'a, Context> CommandRegistry<'a, Context> {
    pub fn register(
        &mut self,
        name: impl Into<String>,
        handler: impl Fn(Value, &mut Context) -> Result<Value, CommandError> + 'a,
    ) {
        let name = name.into();
        assert!(
            !self.handlers.contains_key(&name),
            "duplicate command registration: {name}"
        );
        self.handlers.insert(name, Box::new(handler));
    }
    /// Registers a handler through its definition, making strict input
    /// deserialization and output serialization part of the registry rather
    /// than a concern of individual adapters.
    pub fn register_typed<D: ClientCommandDefinition>(
        &mut self,
        handler: impl Fn(D::Input, &mut Context) -> Result<D::Output, CommandError> + 'a,
    ) {
        self.register(D::NAME, move |arguments, context| {
            let input = serde_json::from_value::<D::Input>(arguments)
                .map_err(|error| CommandError::new("invalid_arguments", error.to_string()))?;
            handler(input, context).and_then(|output| {
                serde_json::to_value(output)
                    .map_err(|error| CommandError::new("internal_command_error", error.to_string()))
            })
        });
    }
    pub fn is_registered(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }
    pub fn dispatch_value(&self, input: Value, context: &mut Context) -> Value {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let parsed = match validate_envelope(input) {
            Ok(value) => value,
            Err(error) => return error_result(command, error.code, error.message),
        };
        let Some(handler) = self.handlers.get(&parsed.command) else {
            return error_result(parsed.command, "unknown_command", "unknown command");
        };
        match handler(parsed.arguments, context) {
            Ok(data) => serde_json::to_value(CommandResult {
                version: COMMAND_VERSION,
                command: parsed.command,
                ok: true,
                data: Some(data),
                error: None,
            })
            .expect("serializable command result"),
            Err(error) => serde_json::to_value(CommandResult {
                version: COMMAND_VERSION,
                command: parsed.command,
                ok: false,
                data: None,
                error: Some(error),
            })
            .expect("serializable command result"),
        }
    }
    pub fn dispatch_json(&self, input: &[u8], context: &mut Context) -> Value {
        if input.len() > MAX_INPUT_BYTES {
            return error_result(
                String::new(),
                "command_input_too_large",
                "command input exceeds 64 KiB",
            );
        }
        match serde_json::from_slice(input) {
            Ok(value) => self.dispatch_value(value, context),
            Err(error) => {
                error_result(String::new(), "invalid_command_envelope", error.to_string())
            }
        }
    }
}

pub fn validate_envelope(input: Value) -> Result<CommandEnvelope, CommandError> {
    let parsed: CommandEnvelope = serde_json::from_value(input)
        .map_err(|error| CommandError::new("invalid_command_envelope", error.to_string()))?;
    if parsed.version != COMMAND_VERSION {
        return Err(CommandError::new(
            "unsupported_command_version",
            format!("version {} is unsupported", parsed.version),
        ));
    }
    if !parsed.arguments.is_object() {
        return Err(CommandError::new(
            "invalid_arguments",
            "arguments must be a JSON object",
        ));
    }
    Ok(parsed)
}

fn error_result(command: String, code: impl Into<String>, message: impl Into<String>) -> Value {
    serde_json::to_value(CommandResult {
        version: COMMAND_VERSION,
        command,
        ok: false,
        data: None,
        error: Some(CommandError::new(code, message)),
    })
    .expect("serializable command error")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct SchemaInput {
        value: u32,
    }
    #[derive(Serialize, JsonSchema)]
    struct SchemaOutput {
        echoed: u32,
    }
    struct EchoDefinition;
    impl ClientCommandDefinition for EchoDefinition {
        type Input = SchemaInput;
        type Output = SchemaOutput;
        const NAME: &'static str = "echo";
        const DEV_LEVEL: u8 = 0;
    }

    #[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
    #[serde(deny_unknown_fields)]
    #[unix(path = "echo")]
    struct EchoUnix {}
    impl ClientCommandDefinition for EchoUnix {
        type Input = Self;
        type Output = serde_json::Value;
        const NAME: &'static str = "echo";
        const DEV_LEVEL: u8 = 0;
    }

    #[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
    #[serde(deny_unknown_fields)]
    #[unix(path = "optional")]
    struct OptionalUnix {
        value: Option<u32>,
    }
    impl ClientCommandDefinition for OptionalUnix {
        type Input = Self;
        type Output = serde_json::Value;
        const NAME: &'static str = "optional";
        const DEV_LEVEL: u8 = 0;
    }
    #[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
    #[serde(deny_unknown_fields)]
    #[unix(path = "many")]
    struct ManyUnix {
        values: Vec<u32>,
    }
    impl ClientCommandDefinition for ManyUnix {
        type Input = Self;
        type Output = serde_json::Value;
        const NAME: &'static str = "many";
        const DEV_LEVEL: u8 = 0;
    }

    #[derive(Deserialize, Serialize, JsonSchema, roundo_proc_macros::UnixCommand)]
    #[serde(deny_unknown_fields)]
    #[unix(path = "server probe")]
    struct OptionsUnix {
        #[unix(positional)]
        host: String,
        #[unix(long, default = "250")]
        interval_ms: u64,
        #[unix(short = 'v', long)]
        verbose: bool,
        #[unix(long = "tag")]
        tags: Vec<String>,
        #[unix(long)]
        filter: Option<String>,
    }
    impl ClientCommandDefinition for OptionsUnix {
        type Input = Self;
        type Output = serde_json::Value;
        const NAME: &'static str = "server.probe";
        const DEV_LEVEL: u8 = 0;
    }

    #[test]
    fn unix_derive_projects_to_json_without_entering_the_dispatcher() {
        assert_eq!(EchoUnix::UNIX_PATH, ["echo"]);
        assert_eq!(EchoUnix::parse_unix(&[]).unwrap(), serde_json::json!({}));
        assert_eq!(
            OptionalUnix::parse_unix(&[]).unwrap(),
            serde_json::json!({"value": null})
        );
        assert_eq!(
            OptionalUnix::parse_unix(&["7".into()]).unwrap(),
            serde_json::json!({"value": 7})
        );
        assert_eq!(
            ManyUnix::parse_unix(&["1".into(), "2".into()]).unwrap(),
            serde_json::json!({"values": [1, 2]})
        );
    }

    #[test]
    fn unix_token_registry_projects_the_longest_canonical_path() {
        let mut registry = UnixCommandRegistry::default();
        registry.register::<OptionsUnix>();
        let (command, arguments) = registry
            .project(&[
                "/SERVER".into(),
                "probe".into(),
                "example.test".into(),
                "--tag".into(),
                "one".into(),
            ])
            .unwrap();
        assert_eq!(command, "server.probe");
        assert_eq!(arguments["tags"], json!(["one"]));
        assert_eq!(registry.usage(&["server".into(), "probe".into()]), Some("server probe <host> [--interval-ms <interval_ms>] [--verbose] [--tag <tags>] [--filter <filter>]".into()));
    }

    #[test]
    fn unix_derive_projects_long_short_bool_default_and_repeated_options() {
        assert_eq!(
            OptionsUnix::parse_unix(&[
                "example.test".into(),
                "--tag".into(),
                "one".into(),
                "-v".into(),
                "--tag".into(),
                "two".into(),
                "--filter".into(),
                "nearby".into(),
            ])
            .unwrap(),
            json!({"host":"example.test","interval_ms":250,"verbose":true,"tags":["one","two"],"filter":"nearby"})
        );
        assert!(OptionsUnix::parse_unix(&["host".into(), "--unknown".into()]).is_err());
    }

    #[test]
    fn unix_tokenizer_handles_quotes_and_rejects_unterminated_syntax() {
        assert_eq!(
            tokenize_unix_line("server add 'Local Server' \"quoted address\"").unwrap(),
            ["server", "add", "Local Server", "quoted address"]
        );
        assert!(tokenize_unix_line("server add 'Local Server").is_err());
        assert!(tokenize_unix_line("server add trailing\\").is_err());
    }

    #[test]
    fn schema_is_generated_from_the_typed_definition() {
        let schema = command_schema::<EchoDefinition>();
        assert_eq!(schema["command"], "echo");
        assert_eq!(schema["input"]["properties"]["command"]["const"], "echo");
        assert!(
            schema["input"]["properties"]["arguments"]["properties"]
                .get("value")
                .is_some()
        );
        assert!(
            schema["success"]["properties"]["data"]["properties"]
                .get("echoed")
                .is_some()
        );
    }

    #[test]
    fn validates_envelope_and_dispatches() {
        let mut registry = CommandRegistry::default();
        registry.register_typed::<EchoDefinition>(|input, _| {
            Ok(SchemaOutput {
                echoed: input.value,
            })
        });
        assert_eq!(
            registry.dispatch_value(
                json!({"version": 1, "command": "echo", "arguments": {"value": 3}}),
                &mut (),
            )["data"]["echoed"],
            3
        );
        assert_eq!(
            registry.dispatch_value(
                json!({"version": 1, "command": "missing", "arguments": {}}),
                &mut ()
            )["error"]["code"],
            "unknown_command"
        );
        assert_eq!(
            registry.dispatch_value(
                json!({"version": 1, "command": "echo", "arguments": [], "extra": true}),
                &mut (),
            )["error"]["code"],
            "invalid_command_envelope"
        );
        assert_eq!(
            registry.dispatch_value(
                json!({"version": 1, "command": "echo", "arguments": {"value": 3, "extra": true}}),
                &mut (),
            )["error"]["code"],
            "invalid_arguments"
        );
    }
    #[test]
    fn rejects_unsupported_version_and_oversized_input() {
        let registry = CommandRegistry::default();
        assert_eq!(
            registry.dispatch_value(
                json!({"version": 2, "command": "x", "arguments": {}}),
                &mut ()
            )["error"]["code"],
            "unsupported_command_version"
        );
        assert_eq!(
            registry.dispatch_json(&vec![b' '; MAX_INPUT_BYTES + 1], &mut ())["error"]["code"],
            "command_input_too_large"
        );
    }
}
