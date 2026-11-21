//! Composes an independent Bevy client World.
use avian3d::PhysicsPlugins;
use bevy::{
    DefaultPlugins,
    app::{App, Plugin, PostStartup},
    log::LogPlugin,
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

/// Installs the windowed ECS runtime on an App with a published voxel registry.
/// Process-wide logging is owned by the host.
pub struct ClientEcsPlugin;

impl Plugin for ClientEcsPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().contains_resource::<AtomicVoxelRegistry>(),
            "ClientEcsPlugin requires AtomicVoxelRegistry"
        );
        app.add_plugins((
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Roundo".to_string(),
                        position: WindowPosition::At(IVec2::new(0, 0)),
                        mode: WindowMode::Windowed,
                        ..default()
                    }),
                    ..default()
                })
                .disable::<LogPlugin>(),
            RoundoRenderingPlugin,
            PhysicsPlugins::default(),
            LocalCoordinatePresentationPlugin,
            MarionetteClientPlugin,
        ));
        app.add_systems(PostStartup, bind_player_controller_to_debug_camera);
    }
}

fn bind_player_controller_to_debug_camera(
    debug_cameras: Query<Entity, With<DebugCamera>>,
    mut player_controller: ResMut<ClientPlayerController>,
) {
    if let Some(debug_camera) = debug_cameras.iter().next() {
        player_controller.bind_camera(debug_camera);
    }
}
