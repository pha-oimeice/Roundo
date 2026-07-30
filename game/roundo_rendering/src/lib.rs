mod debug_camera;

pub use debug_camera::{DebugCamera, DebugCameraPlugin};

use bevy::{
    dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridPlugin, InfiniteGridSettings},
    prelude::{App, Commands, Name, Plugin, Startup, default},
};

pub struct RoundoRenderingPlugin;

impl Plugin for RoundoRenderingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((InfiniteGridPlugin, DebugCameraPlugin))
            .add_systems(Startup, spawn_infinite_grid);
    }
}

fn spawn_infinite_grid(mut commands: Commands) {
    commands.spawn((
        Name::new("Infinite Grid"),
        InfiniteGrid,
        InfiniteGridSettings {
            fadeout_distance: 1_000.0,
            ..default()
        },
    ));
}
