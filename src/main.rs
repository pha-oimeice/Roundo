mod app_process;
mod launcher;

use app_process::AppProcess;
use bevy::{app::AppExit, log::LogPlugin};
use launcher::LaunchTarget;
use roundo_client::RoundoClientPlugin;
use roundo_server::RoundoServerPlugin;

const LAUNCH_TARGETS: &[LaunchTarget] = &[
    LaunchTarget {
        button_label: "Start Server",
        process: AppProcess {
            id: "server",
            configure: |app| {
                app.add_plugins((LogPlugin::default(), RoundoServerPlugin));
            },
        },
    },
    LaunchTarget {
        button_label: "Start Client",
        process: AppProcess {
            id: "client",
            configure: |app| {
                app.add_plugins((LogPlugin::default(), RoundoClientPlugin));
            },
        },
    },
];

fn main() -> AppExit {
    app_process::run(LAUNCH_TARGETS.iter().map(|target| &target.process), || {
        launcher::run(LAUNCH_TARGETS)
    })
}
