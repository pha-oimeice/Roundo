mod config;
use log::{debug, info};
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
    let mut resources =
        roundo_mod_manager::ModLoader::new(roundo_mod_manager::AssetLimits::default())
            .register_prototype(
                roundo_mod_manager::ATOMIC_VOXEL_PROTOTYPE_TYPE,
                &[],
                roundo_local_coordinate::AtomicVoxelRegistry::from_mod_configs,
            )
            .initialize(
                &mods_root,
                &[roundo_mod_manager::ATOMIC_VOXEL_PROTOTYPE_TYPE],
            )
            .unwrap_or_else(|error| {
                panic!(
                    "cannot load Mod Resource Types from {}: {error}",
                    mods_root.display()
                )
            });
    let voxels = resources
        .take_prototype::<roundo_local_coordinate::AtomicVoxelRegistry>(
            roundo_mod_manager::ATOMIC_VOXEL_PROTOTYPE_TYPE,
        )
        .expect("Atomic Voxel Resource Type must be published");
    std::sync::LazyLock::force(&config::SERVER_CONFIG);
    let mut app = ServerEcsRuntime::new(
        voxels,
        ServerEcsConfig {
            tick_rate: config::SERVER_CONFIG.gameplay.tick_rate,
        },
    )
    .into_app();
    // Install explicit diagnostic issuance only. Startup creates no item and
    // does not reserve or advance the persistent identity cursor.
    app.insert_resource(roundo_ecs_entry::debug::DebugItemIssuer::new(|| {
        config::reserve_item_identity()
            .map_err(|error| format!("{error:#}"))?
            .ok_or_else(|| "未配置宿主预留的 item_ids 区间，不能创建物品".into())
    }));
    info!("Standalone server ECS started");
    app.run();
    debug!("Main thread terminated.");
}
