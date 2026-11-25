mod launcher;

use bevy::{app::AppExit, log::LogPlugin, prelude::App};
use launcher::RoundoLauncherPlugin;

fn main() -> AppExit {
    App::new()
        .add_plugins((LogPlugin::default(), RoundoLauncherPlugin))
        .run()
}
