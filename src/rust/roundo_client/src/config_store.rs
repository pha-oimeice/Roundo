//! Client-owned editable settings. Game server connections and probe state are
//! deliberately absent until a new application protocol is installed.

use roundo_user_config::{ClientConfig, ClientSettingsConfig, ConfigDirectory};

#[derive(bevy::prelude::Resource)]
pub(crate) struct ClientConfigStore {
    settings: ClientSettingsConfig,
    directory: ConfigDirectory,
}

impl ClientConfigStore {
    pub(crate) fn new(config: ClientConfig) -> Self {
        Self {
            settings: config.settings,
            directory: ConfigDirectory::executable(),
        }
    }

    pub(crate) fn settings(&self) -> &ClientSettingsConfig {
        &self.settings
    }

    pub(crate) fn settings_mut(&mut self) -> &mut ClientSettingsConfig {
        &mut self.settings
    }

    pub(crate) fn save_setting(&self, key: &str) -> Result<(), String> {
        self.directory.save_client_setting(key, &self.settings)
    }

    pub(crate) fn replace_input_settings(
        &mut self,
        settings: ClientSettingsConfig,
    ) -> Result<(), String> {
        self.directory.save_client_input(&settings)?;
        self.settings.input_slots = settings.input_slots;
        self.settings.input_bindings = settings.input_bindings;
        Ok(())
    }
}
