//! Composes an independent Bevy client World.
use avian3d::PhysicsPlugins;
use bevy::{
    DefaultPlugins,
    app::{App, PostStartup},
    prelude::{
        Entity, IVec2, PluginGroup, Query, ResMut, Window, WindowPlugin, WindowPosition, With,
        default,
    },
    window::WindowMode,
};
use roundo_local_coordinate::{
    AtomicVoxelRegistry, local_coordinate::plugins::LocalCoordinatePresentationPlugin,
};
use roundo_marionette::{ClientPlayerController, MarionetteClientPlugin};
use roundo_rendering::{DebugCamera, RoundoRenderingPlugin};

pub fn create_ecs_client_app(voxels: AtomicVoxelRegistry) -> App {
    let mut app = App::new();
    app.insert_resource(voxels).add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Roundo".to_string(),
                position: WindowPosition::At(IVec2::new(0, 0)),
                mode: WindowMode::Windowed,
                ..default()
            }),
            ..default()
        }),
        RoundoRenderingPlugin,
        PhysicsPlugins::default(),
        LocalCoordinatePresentationPlugin,
        MarionetteClientPlugin,
    ));
    app.add_systems(PostStartup, bind_player_controller_to_debug_camera);
    app
}

pub fn run_ecs_client() {
    create_ecs_client_app(AtomicVoxelRegistry::builtin()).run();
}

fn bind_player_controller_to_debug_camera(
    debug_cameras: Query<Entity, With<DebugCamera>>,
    mut player_controller: ResMut<ClientPlayerController>,
) {
    if let Some(debug_camera) = debug_cameras.iter().next() {
        player_controller.bind_camera(debug_camera);
    }
}
