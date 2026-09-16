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
    let mods_root = config::mod_path();
    let mut resources = roundo_mod_loader::ResourceTypeCatalog::new()
        .register(
            roundo_mod_loader::ATOMIC_VOXEL_RESOURCE_TYPE,
            &[],
            roundo_local_coordinate::AtomicVoxelRegistry::load,
        )
        .register(
            roundo_mod_loader::WEB_UI_RESOURCE_TYPE,
            &[],
            roundo_webui::UiRegistry::load,
        )
        .register(
            roundo_mod_loader::INPUT_RESOURCE_TYPE,
            &[],
            roundo_marionette::InputRegistry::load,
        )
        .load(
            &mods_root,
            &[
                roundo_mod_loader::ATOMIC_VOXEL_RESOURCE_TYPE,
                roundo_mod_loader::WEB_UI_RESOURCE_TYPE,
                roundo_mod_loader::INPUT_RESOURCE_TYPE,
            ],
        )
        .unwrap_or_else(|error| {
            panic!(
                "cannot load Mod Resource Types from {}: {error}",
                mods_root.display()
            )
        });
    let voxels = resources
        .take::<roundo_local_coordinate::AtomicVoxelRegistry>(
            roundo_mod_loader::ATOMIC_VOXEL_RESOURCE_TYPE,
        )
        .expect("Atomic Voxel Resource Type must be published");
    let ui_registry = resources
        .take::<roundo_webui::UiRegistry>(roundo_mod_loader::WEB_UI_RESOURCE_TYPE)
        .expect("Web UI Resource Type must be published");
    let input_registry = resources
        .take::<roundo_marionette::InputRegistry>(roundo_mod_loader::INPUT_RESOURCE_TYPE)
        .expect("Input Resource Type must be published");
    let input_bindings = config::runtime_input_bindings(&settings, &input_registry)
        .unwrap_or_else(|error| panic!("invalid configured input binding: {error}"));
    let resource_fingerprint = roundo_networking::ResourceCatalogFingerprint(voxels.fingerprint());
    let mut app = create_ecs_client_app(voxels);
    // Both Wry and terminal adapters submit to this shared bounded queue; the
    // CLI plugin executes it on Bevy's main world.
    let command_pipe = roundo_cli::ClientCommandPipe::bounded(256);
    let command_io = command_pipe.io();
    app.insert_resource(command_pipe)
        .insert_resource(roundo_cli::client_network::ClientNetworkManager::new(
            config::network_config(),
            client_marionette_ipc(),
            client_presence_ipc(),
            client_local_coordinate_ipc(),
            resource_fingerprint,
        ))
        .insert_resource(config::runtime_input_settings(&settings))
        .insert_resource(input_registry)
        .insert_resource(input_bindings)
        .insert_resource(config::runtime_presence_settings(&settings))
        .insert_resource(roundo_local_coordinate::ClientChunkViewDistance::new(
            settings.world.chunk_view_distance as u16,
        ))
        .insert_resource(targeting::ClientVoxelRaycastSettings::new(
            settings.camera.voxel_raycast_distance,
        ))
        .add_plugins((
            RoundoCliPlugin::client(),
            targeting::ClientVoxelTargetingPlugin,
            roundo_webui::RoundoWebUiPlugin::with_registry_and_command_io(ui_registry, command_io),
            ui_host::ClientWebUiHostPlugin,
        ));
    app.run();
}
