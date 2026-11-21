use bevy::{
    app::{App, Startup},
    log::info,
    prelude::Plugin,
};
use roundo_ecs_entry::ServerEcsPlugin;

mod config;

/// Loads server configuration and Mods, then installs the headless server on
/// the supplied App. Invalid configuration or Mod data aborts installation.
pub struct RoundoServerPlugin;

impl Plugin for RoundoServerPlugin {
    fn build(&self, app: &mut App) {
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
        let tick_rate = config::SERVER_CONFIG.gameplay.tick_rate;
        app.insert_resource(voxels)
            .add_plugins(ServerEcsPlugin { tick_rate });
        // Install explicit diagnostic issuance only. Startup creates no item and
        // does not reserve or advance the persistent identity cursor.
        app.insert_resource(roundo_ecs_entry::debug::DebugItemIssuer::new(|| {
            config::reserve_item_identity()
                .map_err(|error| format!("{error:#}"))?
                .ok_or_else(|| "未配置宿主预留的 item_ids 区间，不能创建物品".into())
        }))
        .add_systems(Startup, || info!("Standalone server ECS started"));
    }
}
