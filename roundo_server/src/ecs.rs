use avian3d::PhysicsPlugins;
use bevy::MinimalPlugins;
use bevy::app::App;
use bevy::mesh::MeshPlugin;
use bevy::prelude::{AssetPlugin, Fixed, Time};
use dynamic_voxel::local_coordinate::plugins::LocalCoordinateServerPlugin;
use roundo_marionette::MarionetteServerPlugin;

pub fn run_ecs_server(marionette: MarionetteServerPlugin, tick_rate: u32) {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(tick_rate.max(1))))
        .add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            MeshPlugin::default(),
            PhysicsPlugins::default(),
            LocalCoordinateServerPlugin,
            marionette,
        ));
    app.run();
}
