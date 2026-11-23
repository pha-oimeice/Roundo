use std::{env, io, io::Write};

const USAGE: &str =
    "Usage: roundo [server|client]\nWith no arguments, choose a mode in the terminal.";

pub(crate) enum LaunchMode {
    Server,
    Client,
}

impl LaunchMode {
    pub(crate) fn select() -> Result<Option<Self>, String> {
        let mut args = env::args_os().skip(1);
        let Some(argument) = args.next() else {
            return prompt_mode().map(Some);
        };
        if args.next().is_some() {
            return Err(format!("Expected a single launch mode.\n{USAGE}"));
        }
        match argument.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                Ok(None)
            }
            Some(mode) => Self::parse(mode).map(Some),
            None => Err(format!("Launch mode is not valid Unicode.\n{USAGE}")),
        }
    }

    fn parse(mode: &str) -> Result<Self, String> {
        match mode {
            "server" => Ok(Self::Server),
            "client" => Ok(Self::Client),
            _ => Err(format!("Unknown launch mode: {mode:?}.\n{USAGE}")),
        }
    }
}

fn prompt_mode() -> Result<LaunchMode, String> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "Choose a launch mode (server/client): ")
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("Could not display launch prompt: {error}"))?;

    let mut line = String::new();
    let bytes_read = io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("Could not read launch mode: {error}"))?;
    if bytes_read == 0 {
        return Err(format!("No launch mode was selected.\n{USAGE}"));
    }
    LaunchMode::parse(line.trim())
}
