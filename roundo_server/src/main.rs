mod config;
mod ecs;
mod my_db;
mod network;

use crate::ecs::run_ecs_server;
use crate::network::start_server;
use log::{debug, info};
use roundo_marionette::MarionetteServerPlugin;

fn main() {
    // load .env file if exists
    dotenvy::dotenv().ok();
    roundo_toolbox::init_logger();
    let marionette = MarionetteServerPlugin::new();
    let marionette_ipc = marionette.ipc();
    start_server(marionette_ipc);
    info!("Server started");
    run_ecs_server(marionette, config::SERVER_CONFIG.gameplay.tick_rate);
    debug!("Main thread terminated.");
}
