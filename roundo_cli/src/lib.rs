//! Source-neutral typed command contracts, dispatch, and terminal projection.
//!
//! This crate deliberately owns no game, networking, configuration, or UI
//! command implementation. Process and domain adapters define commands and
//! register their handlers through [`CommandRegistry`].

extern crate self as roundo_cli;

use bevy::prelude::{App, Plugin, Resource};
use std::{
    collections::VecDeque,
    io::{self, BufRead},
    sync::{Arc, Mutex},
};

pub mod json_command;
pub use json_command::{
    CommandDefinition, CommandEnvelope, CommandError, CommandRegistry, CommandResult, UnixCommand,
    UnixCommandParseError, UnixCommandRegistry,
};
pub use roundo_proc_macros::UnixCommand;

const MAX_COMMANDS_PER_UPDATE: usize = 32;

/// Installs the generic terminal-input adapter.
///
/// Command owners are responsible for draining [`TerminalInput`], projecting
/// registered Unix commands, and dispatching them through [`CommandRegistry`].
pub struct RoundoCliPlugin;

impl Plugin for RoundoCliPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<TerminalInput>() {
            app.insert_resource(TerminalInput::spawn());
        }
    }
}

/// Buffered process stdin shared by command-owner adapters.
#[derive(Resource, Clone)]
pub struct TerminalInput {
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

    /// Creates a deterministic buffered adapter without reading process stdin.
    ///
    /// This is primarily useful to test command-owner plugins through the same
    /// public terminal seam used in production.
    pub fn buffered(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            lines: Arc::new(Mutex::new(lines.into_iter().map(Into::into).collect())),
        }
    }

    /// Removes at most the per-update budget in stdin arrival order.
    pub fn drain(&self) -> Vec<String> {
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = lines.len().min(MAX_COMMANDS_PER_UPDATE);
        lines.drain(..count).collect()
    }
}

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
