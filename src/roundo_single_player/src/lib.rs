//! Windowed single-player host. Game state lives in the main World without a transport.

use bevy::{
    DefaultPlugins,
    app::{App, Plugin, Startup, Update},
    log::{LogPlugin, info},
    prelude::{IVec2, PluginGroup, Window, WindowPlugin, WindowPosition},
    window::WindowMode,
};
use roundo_infrastructure::mod_manager::{
    AssetLimits, ModLoader, UI_PROTOTYPE_TYPE, ui::UiCatalogue,
};
use roundo_local_coordinate::{WorldRenderPlugin, WorldRenderSettings};
use roundo_rendering::RoundoRenderingPlugin;

mod client_data;
mod config;
mod config_store;
mod input;
mod ui_host;

/// Loads configuration and Mods, then installs 3D rendering and the UI runtime.
/// The supplied App must not already have a Bevy runner. Invalid configuration
/// or Mod data aborts installation.
pub struct RoundoSinglePlayerPlugin;

impl Plugin for RoundoSinglePlayerPlugin {
    fn build(&self, app: &mut App) {
        let config = config_store::ClientConfigStore::new(config::load_config());
        let settings = config.settings();
        let mods_root = config::mod_path();
        let mut resources = ModLoader::new(AssetLimits::default())
            .with_ui()
            .initialize(&mods_root, &[UI_PROTOTYPE_TYPE])
            .unwrap_or_else(|error| {
                panic!(
                    "cannot load Mod Resource Types from {}: {error}",
                    mods_root.display()
                )
            });
        let assets = resources.assets();
        let ui_catalogue = resources
            .take_prototype::<UiCatalogue>(UI_PROTOTYPE_TYPE)
            .expect("native UI prototypes must be published");
        ui_host::validate_ui_registry(&ui_catalogue, &assets)
            .unwrap_or_else(|error| panic!("invalid native UI data: {error}"));
        app.add_plugins((
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Roundo".into(),
                        position: WindowPosition::At(IVec2::new(0, 0)),
                        mode: WindowMode::Windowed,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .disable::<LogPlugin>(),
            RoundoRenderingPlugin,
            WorldRenderPlugin,
        ))
        .insert_resource(WorldRenderSettings {
            geometry_lod_pixel_error: settings.world.geometry_lod_pixel_error,
            ..Default::default()
        })
        .insert_resource(config)
        .init_resource::<client_data::HudCache>()
        .add_systems(Startup, || info!("Roundo single-player started"))
        .add_systems(
            Update,
            (
                config_store::sync_runtime_settings,
                client_data::update_hud_cache,
            ),
        )
        .add_plugins((
            roundo_ui::RoundoUiPlugin::new(ui_catalogue, assets),
            input::ClientUiInputPlugin,
            ui_host::ClientUiHostPlugin,
        ));
    }
}
