use log::debug;
use roundo_ecs_entry::create_ecs_client_app;

mod client_data;
mod config;
mod config_store;
mod input;
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

fn main() -> bevy::app::AppExit {
    init_logger();
    debug!("Hello Roundo Client!");
    let config = config_store::ClientConfigStore::new(config::load_config());
    let settings = config.settings();
    let mods_root = config::mod_path();
    let mut resources =
        roundo_mod_manager::ModLoader::new(roundo_mod_manager::AssetLimits::default())
            .register_prototype(
                roundo_mod_manager::ATOMIC_VOXEL_PROTOTYPE_TYPE,
                &[],
                roundo_local_coordinate::AtomicVoxelRegistry::from_mod_configs,
            )
            .with_ui()
            .register_prototype(
                roundo_mod_manager::INPUT_PROTOTYPE_TYPE,
                &[],
                roundo_marionette::InputFunctionRegistry::from_mod_configs,
            )
            .initialize(
                &mods_root,
                &[
                    roundo_mod_manager::ATOMIC_VOXEL_PROTOTYPE_TYPE,
                    roundo_mod_manager::UI_PROTOTYPE_TYPE,
                    roundo_mod_manager::INPUT_PROTOTYPE_TYPE,
                ],
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
    let assets = resources.assets();
    let ui_catalogue = resources
        .take_prototype::<roundo_mod_manager::ui::UiCatalogue>(
            roundo_mod_manager::UI_PROTOTYPE_TYPE,
        )
        .expect("native UI prototypes must be published");
    let input_registry = resources
        .take_prototype::<roundo_marionette::InputFunctionRegistry>(
            roundo_mod_manager::INPUT_PROTOTYPE_TYPE,
        )
        .expect("Input prototypes must be published");
    ui_host::validate_ui_registry(&ui_catalogue, &assets, &input_registry)
        .unwrap_or_else(|error| panic!("invalid native UI data/input binding: {error}"));
    config::validate_input_configuration(settings, &input_registry)
        .unwrap_or_else(|error| panic!("invalid configured input binding: {error}"));
    let mut app = create_ecs_client_app(voxels);
    app.insert_resource(config::runtime_input_settings(settings))
        .insert_resource(input_registry)
        .insert_resource(roundo_rendering::WorldRenderSettings {
            geometry_lod_pixel_error: settings.world.geometry_lod_pixel_error,
            ..Default::default()
        })
        .insert_resource(targeting::ClientVoxelRaycastSettings::new(
            settings.camera.voxel_raycast_distance,
        ))
        .insert_resource(config)
        .init_resource::<client_data::HudCache>()
        .add_systems(
            bevy::prelude::Update,
            (
                config_store::sync_runtime_settings,
                client_data::update_hud_cache,
            ),
        )
        .add_plugins((
            targeting::ClientVoxelTargetingPlugin,
            roundo_ui::RoundoUiPlugin::new(ui_catalogue, assets),
            input::ClientInputPlugin,
            ui_host::ClientUiHostPlugin,
        ));
    app.run()
}
