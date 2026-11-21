use bevy::{
    app::{App, AppExit, Plugins},
    log::LogPlugin,
};
use roundo_client::RoundoClientPlugin;
use roundo_server::RoundoServerPlugin;

mod launcher;

const USAGE: &str =
    "Usage: roundo [server|client]\nWith no arguments, open the graphical launcher.";

#[derive(Clone, Copy)]
enum Role {
    Server,
    Client,
}

impl Role {
    const fn argument(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }

    fn parse(argument: &str) -> Option<Self> {
        [Self::Server, Self::Client]
            .into_iter()
            .find(|role| role.argument() == argument)
    }
}

fn main() -> AppExit {
    let mut args = std::env::args_os().skip(1);
    let Some(argument) = args.next() else {
        return run(launcher::LauncherPlugin);
    };
    if args.next().is_none()
        && let Some(argument) = argument.to_str()
    {
        match Role::parse(argument) {
            Some(Role::Server) => return run(RoundoServerPlugin),
            Some(Role::Client) => return run(RoundoClientPlugin),
            None if matches!(argument, "--help" | "-h") => {
                println!("{USAGE}");
                return AppExit::Success;
            }
            None => {}
        }
    }

    eprintln!("{USAGE}");
    AppExit::error()
}

fn run<M>(plugins: impl Plugins<M>) -> AppExit {
    App::new()
        .add_plugins(LogPlugin::default())
        .add_plugins(plugins)
        .run()
}
