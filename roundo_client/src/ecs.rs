use bevy::DefaultPlugins;
use bevy::app::App;
use bevy::prelude::{IVec2, PluginGroup, Window, WindowPlugin, WindowPosition, default};
use bevy::window::WindowMode;
use dynamic_voxel::local_coordinate::plugins::LocalCoordinateClientPlugin;
use roundo_marionette::MarionetteClientPlugin;

pub fn run_ecs_client(marionette: MarionetteClientPlugin) {
    let mut app = App::new();
    let default_plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Roundo".to_string(),
            position: WindowPosition::At(IVec2::new(0, 0)),
            mode: WindowMode::Windowed,
            ..default()
        }),
        ..default()
    });
    app.add_plugins((default_plugins, LocalCoordinateClientPlugin, marionette));
    app.run();
}
