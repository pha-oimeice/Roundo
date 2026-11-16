//! Category-based configuration persistence. Reads only add status comments;
//! values are changed solely by explicit saves.

use crate::config_document::{ConfigDocument, ReadConfig};
use crate::{
    ClientCameraSettingsConfig, ClientConfig, ClientControlSettingsConfig,
    ClientInputBindingConfig, ClientInputSlotConfig, ClientSettingsConfig,
    ClientWorldSettingsConfig, CommonConfig, GameplayConfig, ItemIdentityConfig, ServerConfig,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// The `config` directory containing `client`, `server`, and `common` categories.
/// Paths supplied to `new` must be roots of this category directory layout.
#[derive(Clone, Debug)]
pub struct ConfigDirectory {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct ProcessConfig {
    dev_mode: bool,
}

#[derive(Serialize, Deserialize)]
struct InputConfig {
    input_slots: BTreeMap<String, ClientInputSlotConfig>,
    input_bindings: Vec<ClientInputBindingConfig>,
}

// Reader-owned declarations. No current field is deprecated; a future reader
// must opt in by overriding DEPRECATED_FIELDS while still consuming that field.
impl ReadConfig for ProcessConfig {}
impl ReadConfig for InputConfig {}
impl ReadConfig for CommonConfig {}
impl ReadConfig for GameplayConfig {}
impl ReadConfig for ItemIdentityConfig {}
impl ReadConfig for ClientControlSettingsConfig {}
impl ReadConfig for ClientCameraSettingsConfig {}
impl ReadConfig for ClientWorldSettingsConfig {}

impl ConfigDirectory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Configuration paths are independent of the process working directory.
    pub fn executable() -> Self {
        Self::new(roundo_toolbox::fs::get_exe_root_path().join("config"))
    }

    pub fn load_common(&self) -> Result<CommonConfig, String> {
        self.read("common/resources.toml")
    }

    pub fn load_client(&self) -> Result<ClientConfig, String> {
        let process: ProcessConfig = self.read("client/process.toml")?;
        let controls = self.read("client/controls.toml")?;
        let camera = self.read("client/camera.toml")?;
        let world = self.read("client/world.toml")?;
        let input: InputConfig = self.read("client/input.toml")?;
        let mut settings = ClientSettingsConfig {
            controls,
            camera,
            world,
            input_slots: input.input_slots,
            input_bindings: input.input_bindings,
        };
        settings.normalize();
        Ok(ClientConfig {
            dev_mode: process.dev_mode,
            settings,
        })
    }

    pub fn load_server(&self) -> Result<ServerConfig, String> {
        let process: ProcessConfig = self.read("server/process.toml")?;
        Ok(ServerConfig {
            dev_mode: process.dev_mode,
            gameplay: self.read("server/gameplay.toml")?,
            item_ids: self.load_item_identity()?,
        })
    }

    pub fn load_item_identity(&self) -> Result<ItemIdentityConfig, String> {
        self.read("server/item-ids.toml")
    }

    /// The caller must hold the installation's item-ID reservation lock from
    /// before reading the checkpoint until after this durable replacement.
    pub fn save_item_identity(&self, config: &ItemIdentityConfig) -> Result<(), String> {
        self.write("server/item-ids.toml", config, None)
    }

    /// Writes only the explicitly edited setting. Other values may have been
    /// normalized or defaulted in memory and must not be copied back implicitly.
    pub fn save_client_setting(
        &self,
        key: &str,
        settings: &ClientSettingsConfig,
    ) -> Result<(), String> {
        let (category, field) = key
            .split_once('.')
            .ok_or_else(|| format!("invalid setting key `{key}`"))?;
        match category {
            "controls" => self.write("client/controls.toml", &settings.controls, Some(field)),
            "camera" => self.write("client/camera.toml", &settings.camera, Some(field)),
            "world" => self.write("client/world.toml", &settings.world, Some(field)),
            _ => Err(format!("unknown setting category for `{key}`")),
        }
    }

    /// Slot resources and binding edges are replaced together in one file.
    pub fn save_client_input(&self, settings: &ClientSettingsConfig) -> Result<(), String> {
        self.write(
            "client/input.toml",
            &InputConfig {
                input_slots: settings.input_slots.clone(),
                input_bindings: settings.input_bindings.clone(),
            },
            None,
        )
    }

    fn read<T: ReadConfig>(&self, relative: &str) -> Result<T, String> {
        let document = ConfigDocument::open(self.root.join(relative))?;
        let config = document.read()?;
        document.save::<T>()?;
        Ok(config)
    }

    fn write<T: ReadConfig>(
        &self,
        relative: &str,
        config: &T,
        field: Option<&str>,
    ) -> Result<(), String> {
        let path = self.root.join(relative);
        let mut document = ConfigDocument::open(path.clone())?;
        document
            .update(config, field)
            .map_err(|error| format!("cannot edit configuration {}: {error}", path.display()))?;
        document.save::<T>()
    }
}
