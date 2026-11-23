use std::{env, io, io::Write};

const SINGLE_PLAYER: &str = "single-player";
const USAGE: &str = "Usage: roundo [single-player]\nWith no arguments, press Enter at the prompt to start single-player.";

pub(crate) fn should_launch() -> Result<bool, String> {
    let mut args = env::args_os().skip(1);
    let Some(argument) = args.next() else {
        return prompt_for_launch();
    };
    if args.next().is_some() {
        return Err(format!("Expected a single launch mode.\n{USAGE}"));
    }
    let argument = argument
        .to_str()
        .ok_or_else(|| format!("Launch mode is not valid Unicode.\n{USAGE}"))?;
    match argument {
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(false)
        }
        mode => parse_mode(mode),
    }
}

fn prompt_for_launch() -> Result<bool, String> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "Launch mode [{SINGLE_PLAYER}]: ")
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("Could not display launch prompt: {error}"))?;
    drop(stdout);

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("Could not read launch mode: {error}"))?;
    parse_mode(&line)
}

fn parse_mode(mode: &str) -> Result<bool, String> {
    match mode.trim() {
        "" | SINGLE_PLAYER => Ok(true),
        mode => Err(format!("Unknown launch mode: {mode:?}.\n{USAGE}")),
    }
}
