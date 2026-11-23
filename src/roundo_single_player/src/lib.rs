//! Windowed single-player host. Game state lives in the main World without a transport.

use std::path::PathBuf;

use bevy::{
    DefaultPlugins,
    app::{App, Plugin, Startup, Update},
    asset::AssetPlugin,
    log::{LogPlugin, info},
    prelude::{IVec2, PluginGroup, Window, WindowPlugin, WindowPosition},
    window::WindowMode,
};
use roundo_infrastructure::toolbox::fs::get_exe_root_path;
use roundo_local_coordinate::{WorldRenderPlugin, WorldRenderSettings};
use roundo_rendering::RoundoRenderingPlugin;

mod config;
mod config_store;
mod launcher_ui;

fn bundled_assets_root() -> PathBuf {
    get_exe_root_path().join("assets")
}

/// Loads client configuration and installs the launcher and 3D rendering.
/// The supplied App must not already have a Bevy runner.
pub struct RoundoSinglePlayerPlugin;

impl Plugin for RoundoSinglePlayerPlugin {
    fn build(&self, app: &mut App) {
        let config = config_store::ClientConfigStore::new(config::load_config());
        let settings = config.settings();
        app.add_plugins((
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: bundled_assets_root()
                        .into_os_string()
                        .into_string()
                        .expect("bundled asset directory must be valid Unicode"),
                    ..Default::default()
                })
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
        .add_systems(Startup, || info!("Roundo single-player started"))
        .add_systems(Update, config_store::sync_runtime_settings)
        .add_plugins(launcher_ui::LauncherUiPlugin);
    }
}
