//! Loaded client settings and their projection into render settings.

use bevy::prelude::{Res, ResMut};
use roundo_infrastructure::user_config::{ClientConfig, ClientSettingsConfig};
use roundo_local_coordinate::WorldRenderSettings;

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
    mut rendering: ResMut<WorldRenderSettings>,
) {
    let settings = config.settings();
    if rendering.geometry_lod_pixel_error != settings.world.geometry_lod_pixel_error {
        rendering.geometry_lod_pixel_error = settings.world.geometry_lod_pixel_error;
    }
}
