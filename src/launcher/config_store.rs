//! Loaded client settings and their projection into render settings.

use bevy::prelude::{Res, ResMut};
use roundo_infrastructure::user_config::{
    ClientConfig, ClientInputBindingConfig, ClientInputKey, ClientSettingsConfig, ConfigDirectory,
    MAX_CHUNK_VIEW_DISTANCE, MIN_CHUNK_VIEW_DISTANCE,
};

use super::local_input::LocalBindings;
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

    pub(super) fn edit_chunk_view_distance(&mut self, radius: f32) -> Result<(), String> {
        if !(MIN_CHUNK_VIEW_DISTANCE..=MAX_CHUNK_VIEW_DISTANCE).contains(&radius)
            || radius.fract() != 0.0
        {
            return Err("Render radius must be a whole number in the allowed range".into());
        }
        if self.settings.world.chunk_view_distance != radius {
            let mut updated = self.settings.clone();
            updated.world.chunk_view_distance = radius;
            ConfigDirectory::executable()
                .save_client_setting("world.chunk_view_distance", &updated)?;
            self.settings.world = updated.world;
        }
        Ok(())
    }

    pub(super) fn edit_binding(
        &mut self,
        action: &str,
        previous: Option<ClientInputKey>,
        replacement: Option<ClientInputKey>,
    ) -> Result<Option<LocalBindings>, String> {
        if !self.settings.input_slots.contains_key(action) {
            return Err(format!("Unknown action `{action}`"));
        }
        let mut updated = self.settings.clone();
        let edges = &mut updated.input_bindings;
        let index = previous
            .map(|key| {
                edges
                    .iter()
                    .position(|edge| edge.slot == action && edge.key == key)
                    .ok_or_else(|| "The binding no longer exists".to_owned())
            })
            .transpose()?;
        if previous == replacement {
            return Ok(None);
        }
        if let Some(key) = replacement {
            if edges
                .iter()
                .any(|edge| edge.slot == action && edge.key == key)
            {
                return Err("This input is already bound to the action".into());
            }
            if let Some(index) = index {
                edges[index].key = key;
            } else {
                edges.push(ClientInputBindingConfig::new(action, key));
            }
        } else if let Some(index) = index {
            edges.remove(index);
        }
        let bindings = LocalBindings::resolve(&updated)?;
        ConfigDirectory::executable().save_client_input(&updated)?;
        self.settings = updated;
        Ok(Some(bindings))
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
