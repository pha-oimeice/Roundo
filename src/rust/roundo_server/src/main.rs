mod commands;
mod config;
mod my_db;
mod network;

use crate::network::{DatabaseSessionProvider, ServerNetworkRuntime, network_config};
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
    std::sync::LazyLock::force(&config::SERVER_CONFIG);
    let item_config = &config::SERVER_CONFIG.block_item;
    for reach in [item_config.placement_reach, item_config.destruction_reach]
        .into_iter()
        .flatten()
    {
        assert!(
            reach.is_finite() && reach > 0.0,
            "block item reach must be finite and positive"
        );
    }
    let mut item_issuer = config::reserve_block_item_identity()
        .expect("reserve the block item's instance identity before startup");
    let (mut app, ecs) = ServerEcsRuntime::new(
        voxels,
        ServerEcsConfig {
            tick_rate: config::SERVER_CONFIG.gameplay.tick_rate,
            presence_radius: config::SERVER_CONFIG.gameplay.presence_radius,
        },
    )
    .into_parts();
    if let Some(issuer) = item_issuer.as_mut() {
        roundo_ecs_entry::create_block_item(
            app.world_mut(),
            issuer,
            item_config
                .placement_reach
                .map(|reach| roundo_block_interaction::BlockPlacementPermit { reach }),
            item_config
                .destruction_reach
                .map(|reach| roundo_block_interaction::BlockDestructionPermit { reach }),
        )
        .expect("initialize the composed block item");
    } else {
        log::warn!("block item is not configured: supply a host-reserved item_ids interval");
    }
    let _network_runtime = ServerNetworkRuntime::start(
        network_config(),
        std::sync::Arc::new(DatabaseSessionProvider),
        ecs.marionette,
        ecs.presence,
        ecs.local_coordinate,
        ecs.player_control,
        resource_fingerprint,
    );
    info!("Server started");
    app.add_plugins((RoundoCliPlugin, commands::ServerCommandsPlugin))
        .run();
    debug!("Main thread terminated.");
}
