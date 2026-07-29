use crate::{ecs::run_ecs_client, network::start_client};
use log::debug;
use roundo_marionette::MarionetteClientPlugin;

mod config;
mod ecs;
mod network;

fn main() {
    roundo_toolbox::init_logger();
    debug!("Hello Roundo Client!");
    let marionette = MarionetteClientPlugin::new();
    let marionette_ipc = marionette.ipc();
    start_client(marionette_ipc);
    run_ecs_client(marionette);
}
