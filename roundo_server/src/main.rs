mod config;
mod my_db;
mod network;

use crate::network::start_server;
use log::{debug, info};
use roundo_cli::RoundoCliPlugin;
use roundo_ecs_entry::{
    configure_server_presence_radius, configure_server_tick_rate, create_ecs_server_app,
    server_local_coordinate_ipc, server_marionette_ipc, server_presence_ipc,
};

fn main() {
    roundo_toolbox::init_logger();
    // Load the one installation-wide configuration shared with the client.
    std::sync::LazyLock::force(&config::COMMON_CONFIG);
    configure_server_tick_rate(config::SERVER_CONFIG.gameplay.tick_rate);
    configure_server_presence_radius(config::SERVER_CONFIG.gameplay.presence_radius);
    start_server(
        server_marionette_ipc(),
        server_presence_ipc(),
        server_local_coordinate_ipc(),
    );
    info!("Server started");
    let mut app = create_ecs_server_app();
    app.add_plugins(RoundoCliPlugin::server()).run();
    debug!("Main thread terminated.");
}
