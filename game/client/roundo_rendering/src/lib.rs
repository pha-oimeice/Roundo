mod debug_camera;
mod render_object;

pub use debug_camera::{DebugCamera, DebugCameraPlugin};
pub use render_object::{
    RenderMaterial, RenderMesh, RenderMeshError, RenderObject, RenderObjectId, RenderObjectPlugin,
    RenderObjectSync, RenderObjects, RenderTransform, issue_render_object, remove_render_object,
    render_object, render_object_material, render_object_mesh, render_object_transform,
    update_render_object_material, update_render_object_mesh, update_render_object_name,
    update_render_object_transform, update_render_object_visibility,
};

use bevy::{
    dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridPlugin, InfiniteGridSettings},
    prelude::{App, Commands, Name, Plugin, Startup, default},
};

pub struct RoundoRenderingPlugin;

impl Plugin for RoundoRenderingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RenderObjectPlugin, InfiniteGridPlugin, DebugCameraPlugin))
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
