use bevy::{
    DefaultPlugins,
    app::{App, Plugin},
    log::LogPlugin,
    prelude::{IVec2, PluginGroup, Window, WindowPlugin, WindowPosition},
    window::WindowMode,
};
use roundo_infrastructure::mod_manager::{
    AssetLimits, ModLoader, UI_PROTOTYPE_TYPE, ui::UiCatalogue,
};
use roundo_rendering::RoundoRenderingPlugin;

mod client_data;
mod config;
mod config_store;
mod input;
mod ui_host;

/// Loads client configuration and Mods, then installs the windowed client on
/// the supplied App. Invalid configuration or Mod data aborts installation.
pub struct RoundoClientPlugin;

impl Plugin for RoundoClientPlugin {
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
        ))
        .insert_resource(roundo_rendering::WorldRenderSettings {
            geometry_lod_pixel_error: settings.world.geometry_lod_pixel_error,
            ..Default::default()
        })
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
            roundo_ui::RoundoUiPlugin::new(ui_catalogue, assets),
            input::ClientUiInputPlugin,
            ui_host::ClientUiHostPlugin,
        ));
    }
}
