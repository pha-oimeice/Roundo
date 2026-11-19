//! Native UI camera and host settings.

use crate::targeting::ClientVoxelRaycastSettings;
use bevy::prelude::*;
use roundo_marionette::{ClientMarionetteInputSettings, ClientPlayerController};
pub struct ClientUiHostPlugin;
impl Plugin for ClientUiHostPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_ui_camera)
            .add_systems(PostStartup, disable_world_input_and_rendering)
            .add_systems(
                Update,
                sync_runtime_settings
                    .after(crate::commands::ClientCommandSystemSet::ExecuteAndPublish),
            );
    }
}

// The stock client only mounts the menu; the debug camera is spawned in Startup.
fn disable_world_input_and_rendering(
    mut controller: ResMut<ClientPlayerController>,
    mut cameras: Query<&mut Camera, With<Camera3d>>,
) {
    controller.set_input_enabled(false);
    for mut camera in &mut cameras {
        camera.is_active = false;
    }
}

fn spawn_ui_camera(mut commands: Commands) {
    commands.spawn((
        bevy::ui::IsDefaultUiCamera,
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
    ));
}
fn sync_runtime_settings(
    config: Res<crate::config_store::ClientConfigStore>,
    mut input: ResMut<ClientMarionetteInputSettings>,
    mut targeting: ResMut<ClientVoxelRaycastSettings>,
    mut rendering: ResMut<roundo_rendering::WorldRenderSettings>,
) {
    let settings = config.settings();
    input.mouse_sensitivity = settings.controls.mouse_sensitivity;
    input.camera_move_speed = settings.camera.move_speed;
    targeting.set_max_distance(settings.camera.voxel_raycast_distance);
    if rendering.geometry_lod_pixel_error != settings.world.geometry_lod_pixel_error {
        rendering.geometry_lod_pixel_error = settings.world.geometry_lod_pixel_error;
    }
}
