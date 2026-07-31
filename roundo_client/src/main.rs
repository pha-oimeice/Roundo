use crate::ui::RoundoClientUiPlugin;
use log::debug;
use roundo_ecs_entry::{
    client_character_ipc, client_local_coordinate_ipc, client_marionette_ipc, create_ecs_client_app,
};

mod config;
mod network;
mod targeting;
mod ui;

fn main() {
    roundo_toolbox::init_logger();
    debug!("Hello Roundo Client!");
    let settings = config::settings();
    let mut app = create_ecs_client_app();
    app.insert_resource(config::runtime_input_settings(&settings))
        .insert_resource(config::runtime_key_bindings(&settings))
        .insert_resource(targeting::ClientVoxelRaycastSettings::new(
            settings.camera.voxel_raycast_distance,
        ))
        .add_plugins((
            targeting::ClientVoxelTargetingPlugin,
            RoundoClientUiPlugin::new(
                client_marionette_ipc(),
                client_character_ipc(),
                client_local_coordinate_ipc(),
            ),
        ));
    app.run();
}
