mod config;
mod my_db;
mod network;

use crate::network::start_server;
use log::{debug, info};
use roundo_ecs_entry::{
    configure_server_presence_radius, configure_server_tick_rate, run_ecs_server,
    server_local_coordinate_ipc, server_marionette_ipc, server_presence_ipc,
};

fn main() {
    // load .env file if exists
    dotenvy::dotenv().ok();
    roundo_toolbox::init_logger();
    configure_server_tick_rate(config::SERVER_CONFIG.gameplay.tick_rate);
    configure_server_presence_radius(config::SERVER_CONFIG.gameplay.presence_radius);
    start_server(
        server_marionette_ipc(),
        server_presence_ipc(),
        server_local_coordinate_ipc(),
    );
    info!("Server started");
    run_ecs_server();
    debug!("Main thread terminated.");
}
