//! Window, camera and cursor projection for Bevy's native UI.

use crate::scene::{ClientSceneProjection, ClientSceneSystemSet};
use crate::targeting::ClientVoxelRaycastSettings;
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use roundo_marionette::{ClientMarionetteInputSettings, ClientPlayerController};
use roundo_presence::ClientPresenceSettings;

#[derive(Default, Resource)]
struct AppliedHostProjection(Option<(Option<roundo_ui::UiInstanceId>, bool, bool)>);
#[derive(Component)]
struct UiCamera;
pub struct ClientUiHostPlugin;
impl Plugin for ClientUiHostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppliedHostProjection>()
            .add_systems(Startup, spawn_ui_camera)
            .add_systems(
                Update,
                sync_runtime_settings
                    .after(crate::commands::ClientCommandSystemSet::ExecuteAndPublish)
                    .before(ClientSceneSystemSet::Requests),
            )
            .add_systems(
                Update,
                apply_host_projection
                    .run_if(resource_exists::<roundo_ui::UiReady>)
                    .after(ClientSceneSystemSet::Project)
                    .before(roundo_marionette::MarionetteClientSet::Input),
            );
    }
}
fn apply_host_projection(
    projection: Res<ClientSceneProjection>,
    mut applied: ResMut<AppliedHostProjection>,
    mut controller: ResMut<ClientPlayerController>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut cameras: Query<(Entity, &mut Camera), (With<Camera3d>, Without<UiCamera>)>,
    mut ui_camera: Query<&mut Camera, With<UiCamera>>,
) {
    for (entity, mut camera) in &mut cameras {
        camera.is_active = projection.world_active && controller.is_bound_to(entity);
    }
    for mut camera in &mut ui_camera {
        camera.clear_color = if projection.world_active {
            ClearColorConfig::None
        } else {
            ClearColorConfig::Custom(Color::BLACK)
        };
    }
    let key = (
        projection.focused,
        projection.world_active,
        projection.gameplay_input,
    );
    if applied.0 == Some(key) {
        return;
    }
    controller.set_input_enabled(projection.gameplay_input);
    for (mut window, mut cursor) in &mut windows {
        cursor.visible = !projection.gameplay_input;
        cursor.grab_mode = if projection.gameplay_input {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if projection.gameplay_input {
            let center = Vec2::new(window.width() * 0.5, window.height() * 0.5);
            window.set_cursor_position(Some(center));
        }
    }
    applied.0 = Some(key);
}
fn spawn_ui_camera(mut commands: Commands) {
    commands.spawn((
        UiCamera,
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
    mut presence: ResMut<ClientPresenceSettings>,
    mut chunk_view_distance: ResMut<roundo_local_coordinate::ClientChunkViewDistance>,
    mut targeting: ResMut<ClientVoxelRaycastSettings>,
    mut rendering: ResMut<roundo_rendering::WorldRenderSettings>,
) {
    let settings = config.settings();
    input.mouse_sensitivity = settings.controls.mouse_sensitivity;
    input.camera_move_speed = settings.camera.move_speed;
    input.player_movement_prediction = settings.controls.player_movement_prediction;
    presence.set_joinable_world_radius(settings.world.joinable_world_radius);
    let configured_view_distance = settings.world.chunk_view_distance as u16;
    if chunk_view_distance.chunks() != configured_view_distance {
        chunk_view_distance.set_chunks(configured_view_distance);
    }
    targeting.set_max_distance(settings.camera.voxel_raycast_distance);
    if rendering.geometry_lod_pixel_error != settings.world.geometry_lod_pixel_error {
        rendering.geometry_lod_pixel_error = settings.world.geometry_lod_pixel_error;
    }
}
