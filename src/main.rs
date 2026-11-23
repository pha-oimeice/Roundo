mod launcher;

use bevy::{app::AppExit, log::LogPlugin, prelude::App};
use roundo_single_player::RoundoSinglePlayerPlugin;

fn main() -> AppExit {
    match launcher::should_launch() {
        Ok(true) => {}
        Ok(false) => return AppExit::Success,
        Err(error) => {
            eprintln!("{error}");
            return AppExit::error();
        }
    }

    App::new()
        .add_plugins((LogPlugin::default(), RoundoSinglePlayerPlugin))
        .run()
}
