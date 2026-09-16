mod config;
mod my_db;
mod network;

use crate::network::ServerNetworkRuntime;
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
    let mods_root = config::COMMON_CONFIG.resolved_mod_path();
    let mut resources = roundo_mod_loader::ResourceTypeCatalog::new()
        .register(
            roundo_mod_loader::ATOMIC_VOXEL_RESOURCE_TYPE,
            &[],
            roundo_local_coordinate::AtomicVoxelRegistry::load,
        )
        .load(&mods_root, &[roundo_mod_loader::ATOMIC_VOXEL_RESOURCE_TYPE])
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
    let resource_fingerprint = roundo_networking::ResourceCatalogFingerprint(voxels.fingerprint());
    let _network_runtime = ServerNetworkRuntime::start(
        server_marionette_ipc(),
        server_presence_ipc(),
        server_local_coordinate_ipc(),
        resource_fingerprint,
    );
    info!("Server started");
    let mut app = create_ecs_server_app(voxels);
    app.add_plugins(RoundoCliPlugin::server()).run();
    debug!("Main thread terminated.");
}
