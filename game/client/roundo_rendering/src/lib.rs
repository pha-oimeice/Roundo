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

use bevy::prelude::{App, Plugin};

pub struct RoundoRenderingPlugin;

impl Plugin for RoundoRenderingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RenderObjectPlugin, DebugCameraPlugin));
    }
}
