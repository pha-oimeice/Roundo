use crate::ui::RoundoClientUiPlugin;
use log::debug;
use roundo_ecs_entry::{
    client_character_ipc, client_marionette_ipc, client_static_voxel_ipc, create_ecs_client_app,
};

mod config;
mod network;
mod ui;

fn main() {
    roundo_toolbox::init_logger();
    debug!("Hello Roundo Client!");
    let mut app = create_ecs_client_app();
    app.add_plugins(RoundoClientUiPlugin::new(
        client_marionette_ipc(),
        client_character_ipc(),
        client_static_voxel_ipc(),
    ));
    app.run();
}
