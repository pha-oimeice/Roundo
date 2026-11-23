mod launcher;

use bevy::{app::AppExit, log::LogPlugin, prelude::App};
use launcher::LaunchMode;
use roundo_client::RoundoClientPlugin;
use roundo_server::RoundoServerPlugin;

fn main() -> AppExit {
    let mode = match LaunchMode::select() {
        Ok(Some(mode)) => mode,
        Ok(None) => return AppExit::Success,
        Err(error) => {
            eprintln!("{error}");
            return AppExit::error();
        }
    };

    let mut app = App::new();
    app.add_plugins(LogPlugin::default());
    match mode {
        LaunchMode::Server => app.add_plugins(RoundoServerPlugin),
        LaunchMode::Client => app.add_plugins(RoundoClientPlugin),
    };
    app.run()
}
