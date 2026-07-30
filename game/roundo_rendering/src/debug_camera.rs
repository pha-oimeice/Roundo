use bevy::prelude::{App, Camera3d, Commands, Component, Name, Plugin, Startup, Transform, Vec3};

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct DebugCamera;

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
