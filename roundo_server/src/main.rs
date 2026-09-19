mod commands;
mod config;
mod my_db;
mod network;

use crate::network::ServerNetworkRuntime;
use log::{debug, info};
use roundo_cli::RoundoCliPlugin;
use roundo_ecs_entry::{ServerEcsConfig, ServerEcsRuntime};

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
    // Load the one installation-wide configuration shared with the client.
    std::sync::LazyLock::force(&config::COMMON_CONFIG);
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
    let resource_fingerprint = roundo_contracts::ResourceCatalogFingerprint(voxels.fingerprint());
    let (mut app, ecs) = ServerEcsRuntime::new(
        voxels,
        ServerEcsConfig {
            tick_rate: config::SERVER_CONFIG.gameplay.tick_rate,
            presence_radius: config::SERVER_CONFIG.gameplay.presence_radius,
        },
    )
    .into_parts();
    let _network_runtime = ServerNetworkRuntime::start(
        ecs.marionette,
        ecs.presence,
        ecs.local_coordinate,
        resource_fingerprint,
    );
    info!("Server started");
    app.add_plugins((RoundoCliPlugin, commands::ServerCommandsPlugin))
        .run();
    debug!("Main thread terminated.");
}
