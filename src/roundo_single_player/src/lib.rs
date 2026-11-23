//! Windowed single-player host. Game state lives in the main World without a transport.

use bevy::{
    DefaultPlugins,
    app::{App, Plugin, Startup, Update},
    log::{LogPlugin, info},
    prelude::{IVec2, PluginGroup, Window, WindowPlugin, WindowPosition},
    window::WindowMode,
};
use roundo_local_coordinate::{WorldRenderPlugin, WorldRenderSettings};
use roundo_rendering::RoundoRenderingPlugin;

mod config;
mod config_store;
mod launcher_ui;

/// Loads client configuration and installs the launcher and 3D rendering.
/// The supplied App must not already have a Bevy runner.
pub struct RoundoSinglePlayerPlugin;

impl Plugin for RoundoSinglePlayerPlugin {
    fn build(&self, app: &mut App) {
        let config = config_store::ClientConfigStore::new(config::load_config());
        let settings = config.settings();
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
        .add_systems(Startup, || info!("Roundo single-player started"))
        .add_systems(Update, config_store::sync_runtime_settings)
        .add_plugins(launcher_ui::LauncherUiPlugin);
    }
}
