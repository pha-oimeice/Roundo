use log::debug;
use roundo_cli::RoundoCliPlugin;
use roundo_ecs_entry::ClientEcsRuntime;

mod commands;
mod config;
mod network;
mod targeting;
mod ui_host;

fn init_logger() {
    let filter_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    env_logger::Builder::new()
        .default_format()
        .format_level(true)
        .filter_module("marionette", filter_level)
        .filter_module("roundo", filter_level)
        .write_style(env_logger::WriteStyle::Always)
        .init();
}

fn main() {
    init_logger();
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
    let resource_fingerprint = roundo_contracts::ResourceCatalogFingerprint(voxels.fingerprint());
    let (mut app, ecs) = ClientEcsRuntime::new(voxels).into_parts();
    // Both Wry and terminal adapters submit to this shared bounded queue; the
    // client-owned command plugin executes it on Bevy's main world.
    let command_pipe = commands::ClientCommandPipe::bounded(256);
    let command_io = command_pipe.io();
    app.insert_resource(command_pipe)
        .insert_resource(network::ClientNetworkManager::new(
            config::network_config(),
            ecs.marionette,
            ecs.presence,
            ecs.local_coordinate,
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
            RoundoCliPlugin,
            commands::ClientCommandsPlugin,
            targeting::ClientVoxelTargetingPlugin,
            roundo_webui::RoundoWebUiPlugin::with_registry_and_command_io(ui_registry, command_io),
            ui_host::ClientWebUiHostPlugin,
        ));
    app.run();
}
