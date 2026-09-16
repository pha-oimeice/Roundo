//! Client rendering plugins for world geometry, presence, and render objects.

mod debug_camera;
mod presence_visual;
mod render_object;
mod world;

pub use debug_camera::{DebugCamera, DebugCameraPlugin};
pub use presence_visual::PresenceVisualPlugin;
pub use render_object::{
    RenderMaterial, RenderMesh, RenderMeshError, RenderObject, RenderObjectId, RenderObjectPlugin,
    RenderObjectSync, RenderObjects, RenderTransform, issue_render_object, remove_render_object,
    render_object, render_object_material, render_object_mesh, render_object_transform,
    update_render_object_material, update_render_object_mesh, update_render_object_name,
    update_render_object_transform, update_render_object_visibility,
};
pub use world::{WorldRenderPlugin, WorldRenderSettings};

use bevy::prelude::{App, Plugin};

/// Installs the complete rendering stack in dependency order.
pub struct RoundoRenderingPlugin;

// Individual plugins retain ownership of their extraction and update systems.
impl Plugin for RoundoRenderingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            RenderObjectPlugin,
            PresenceVisualPlugin,
            WorldRenderPlugin,
            DebugCameraPlugin,
        ));
    }
}
