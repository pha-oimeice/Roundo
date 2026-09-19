//! Typed JSON command definitions with Web UI and optional Unix-terminal adapters.
//!
//! Client WebViews submit bounded request/response calls; terminal input is a
//! separate text projection into the same canonical command envelopes.

extern crate self as roundo_cli;

use bevy::prelude::{App, Plugin, Resource};
#[cfg(feature = "client")]
use roundo_toolbox::request_response_pipe::{
    CommandTransport, ContextualJsonRequestResponseIo, RequestResponsePipe,
};
#[cfg(feature = "client")]
use roundo_webui::UiCommandSource;
#[cfg(feature = "client")]
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{self, BufRead},
    sync::{Arc, Mutex},
};

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::{ClientConfigStore, HudCache};
#[cfg(feature = "client")]
pub mod client_network;
pub mod json_command;
pub use json_command::{
    ClientCommandDefinition, UnixCommand, UnixCommandParseError, UnixCommandRegistry,
};
pub use roundo_proc_macros::UnixCommand;
#[cfg(feature = "server")]
mod server;

// Prevents an unbounded terminal backlog from monopolizing one ECS update.
const MAX_COMMANDS_PER_UPDATE: usize = 32;
/// Maximum queued JSON commands dispatched during one client ECS update.
pub const MAX_JSON_COMMANDS_PER_UPDATE: usize = 64;

/// ECS-owned consumer side of the bounded client command queue.
#[cfg(feature = "client")]
#[derive(Resource)]
pub struct ClientCommandPipe(RequestResponsePipe<CommandTransport<UiCommandSource>, Value>);
#[cfg(feature = "client")]
impl ClientCommandPipe {
    /// Creates a shared request queue with exactly `capacity` slots.
    ///
    /// A zero capacity is a rendezvous queue; non-blocking submission then
    /// succeeds only while a receiver is waiting.
    pub fn bounded(capacity: usize) -> Self {
        Self(RequestResponsePipe::bounded(capacity))
    }
    /// Returns a producer sharing this pipe's bounded queue.
    pub fn io(&self) -> ClientCommandIo {
        ContextualJsonRequestResponseIo::new(self.0.io())
    }
    /// Attempts to dequeue one command and its private response sender.
    ///
    /// `None` means either currently empty or all producers disconnected.
    pub fn try_receive(
        &self,
    ) -> Option<(
        CommandTransport<UiCommandSource>,
        roundo_toolbox::request_response_pipe::ResponseSender<Value>,
    )> {
        self.0.try_receive()
    }
}
/// The only client-command submission adapter. It rejects serialized JSON
/// inputs larger than 64 KiB before they can enter the command queue while
/// preserving adapter authority as transport context, not command JSON.
#[cfg(feature = "client")]
pub type ClientCommandIo = ContextualJsonRequestResponseIo<UiCommandSource, Value>;

/// Main-world phase that executes accepted client commands and publishes
/// Client Data snapshots before the WebUI platform phase polls responses or
/// creates another WebView2 controller.
#[cfg(feature = "client")]
#[derive(Clone, Debug, Eq, Hash, PartialEq, bevy::prelude::SystemSet)]
pub enum ClientCommandSystemSet {
    ExecuteAndPublish,
}

/// Feature-selected CLI composition for either client or server process.
///
/// Installation also starts one detached stdin reader. The reader may remain
/// blocked in OS input after the Bevy app exits; there is no cancellation API.
pub struct RoundoCliPlugin {
    configure: fn(&mut App),
}

impl RoundoCliPlugin {
    /// Creates the client command plugin with typed JSON/Web UI dispatch.
    #[cfg(feature = "client")]
    pub const fn client() -> Self {
        Self {
            configure: client::configure,
        }
    }

    /// Creates the server-console command plugin.
    #[cfg(feature = "server")]
    pub const fn server() -> Self {
        Self {
            configure: server::configure,
        }
    }
}

impl Plugin for RoundoCliPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TerminalInput::spawn());
        (self.configure)(app);
    }
}

#[derive(Resource, Clone)]
struct TerminalInput {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl TerminalInput {
    fn spawn() -> Self {
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let terminal_lines = Arc::clone(&lines);
        if let Err(error) = std::thread::Builder::new()
            .name("roundo-terminal-input".to_string())
            .spawn(move || read_terminal_lines(terminal_lines))
        {
            eprintln!("[cli] failed to start terminal input: {error}");
        }
        Self { lines }
    }

    /// Removes at most the per-update budget in stdin arrival order.
    fn drain(&self) -> Vec<String> {
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = lines.len().min(MAX_COMMANDS_PER_UPDATE);
        lines.drain(..count).collect()
    }

    #[cfg(test)]
    fn buffered(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            lines: Arc::new(Mutex::new(lines.into_iter().map(Into::into).collect())),
        }
    }
}

// Blocks on stdin and appends complete lines until EOF or the first read error.
fn read_terminal_lines(lines: Arc<Mutex<VecDeque<String>>>) {
    for line in io::stdin().lock().lines() {
        match line {
            Ok(line) => lines
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_back(line),
            Err(error) => {
                eprintln!("[cli] failed to read terminal input: {error}");
                break;
            }
        }
    }
}
