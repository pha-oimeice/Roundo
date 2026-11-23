//! Rendering plugins for Creature visuals, render objects, and the debug camera.

mod creature_visual;
mod debug_camera;
mod render_object;

pub use creature_visual::{CreatureVisualPlugin, LocalCreatureDebugVisual};
pub use debug_camera::{DebugCamera, DebugCameraPlugin};
pub use render_object::{
    RenderMaterial, RenderMesh, RenderMeshError, RenderObject, RenderObjectId, RenderObjectPlugin,
    RenderObjectSync, RenderObjects, RenderTransform, issue_render_object, remove_render_object,
    render_object, render_object_material, render_object_mesh, render_object_transform,
    update_render_object_material, update_render_object_mesh, update_render_object_name,
    update_render_object_transform, update_render_object_visibility,
};

use bevy::prelude::{App, Plugin};

/// Installs object rendering, Creature debug visuals, and the debug camera.
pub struct RoundoRenderingPlugin;

// Individual plugins retain ownership of their extraction and update systems.
impl Plugin for RoundoRenderingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RenderObjectPlugin, CreatureVisualPlugin, DebugCameraPlugin));
    }
}
