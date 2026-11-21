//! Loaded client settings and their projection into runtime resources.

use crate::targeting::ClientVoxelRaycastSettings;
use bevy::prelude::{Res, ResMut};
use roundo_marionette::ClientMarionetteInputSettings;
use roundo_user_config::{ClientConfig, ClientSettingsConfig};

#[derive(bevy::prelude::Resource)]
pub(crate) struct ClientConfigStore {
    settings: ClientSettingsConfig,
}

impl ClientConfigStore {
    pub(crate) fn new(config: ClientConfig) -> Self {
        Self {
            settings: config.settings,
        }
    }

    pub(crate) fn settings(&self) -> &ClientSettingsConfig {
        &self.settings
    }
}

pub(crate) fn sync_runtime_settings(
    config: Res<ClientConfigStore>,
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
