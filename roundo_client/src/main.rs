use log::debug;
use roundo_cli::RoundoCliPlugin;
use roundo_ecs_entry::{
    client_local_coordinate_ipc, client_marionette_ipc, client_presence_ipc, create_ecs_client_app,
};

mod config;
mod targeting;
mod ui_host;

fn main() {
    roundo_toolbox::init_logger();
    debug!("Hello Roundo Client!");
    let settings = config::settings();
    let mut app = create_ecs_client_app();
    // Both Wry and terminal adapters submit to this shared bounded queue; the
    // CLI plugin executes it on Bevy's main world.
    let command_pipe = roundo_cli::ClientCommandPipe::bounded(256);
    let command_io = command_pipe.io();
    app.insert_resource(command_pipe)
        .insert_resource(roundo_cli::client_network::ClientNetworkManager::new(
            client_marionette_ipc(),
            client_presence_ipc(),
            client_local_coordinate_ipc(),
        ))
        .insert_resource(config::runtime_input_settings(&settings))
        .insert_resource(config::runtime_key_bindings(&settings))
        .insert_resource(config::runtime_presence_settings(&settings))
        .insert_resource(targeting::ClientVoxelRaycastSettings::new(
            settings.camera.voxel_raycast_distance,
        ))
        .add_plugins((
            RoundoCliPlugin::client(),
            targeting::ClientVoxelTargetingPlugin,
            roundo_webui::RoundoWebUiPlugin::with_mods_root_and_command_io(
                config::mod_path(),
                command_io,
            ),
            ui_host::ClientWebUiHostPlugin,
        ));
    app.run();
}
