//! Development camera setup for inspecting a scene without player state.

use bevy::prelude::{App, Camera3d, Commands, Component, Name, Plugin, Startup, Transform, Vec3};

#[derive(Component, Clone, Copy, Debug, Default)]
/// Marks the camera created by [`DebugCameraPlugin`].
pub struct DebugCamera;

/// Installs a single startup camera with an origin-facing transform.
pub struct DebugCameraPlugin;

impl Plugin for DebugCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_debug_camera);
    }
}

fn spawn_debug_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("Debug Camera"),
        DebugCamera,
        Camera3d::default(),
        Transform::from_xyz(8.0, 8.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
