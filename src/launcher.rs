use std::env;

const SINGLE_PLAYER: &str = "single-player";
const USAGE: &str =
    "Usage: roundo_bin [single-player]\nWith no arguments, opens the graphical launcher.";

pub(crate) fn should_launch() -> Result<bool, String> {
    let mut args = env::args_os().skip(1);
    let Some(argument) = args.next() else {
        return Ok(true);
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
        SINGLE_PLAYER => Ok(true),
        mode => Err(format!("Unknown launch mode: {mode:?}.\n{USAGE}")),
    }
}
