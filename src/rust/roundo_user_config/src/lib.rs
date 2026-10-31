//! Persistent client, server, network, database, and gameplay configuration.

mod client_settings;
mod my_default_configurations;
mod my_impls;
mod process_config;

pub use client_settings::*;
pub use process_config::*;

use roundo_toolbox::fs::get_exe_root_path;
use roundo_toolbox::load_or_create_config;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const COMMON_CONFIG_FILE_NAME: &str = "roundo-common-config.toml";

/// Loads the shared configuration file from the executable root.
pub fn load_common_config() -> CommonConfig {
    load_config(COMMON_CONFIG_FILE_NAME)
}

/// Loads or creates a typed TOML configuration file.
pub fn load_config<T>(config_file_name: &str) -> T
where
    T: Serialize + DeserializeOwned + Default,
{
    load_or_create_config::<T>(&get_exe_root_path(), config_file_name).unwrap_or_else(|e| {
        eprintln!(
            "Error loading config: {}. Using default configuration.",
            e.err_msg
        );
        e.data
            .unwrap_or_else(|| unreachable!("Default configuration should always be available."))
    })
}

/// Loads client configuration and migrates legacy settings when required.
pub fn load_client_config(config_file_name: &str) -> ClientConfig {
    load_client_config_at(&get_exe_root_path(), config_file_name)
}

fn load_client_config_at(directory: &std::path::Path, config_file_name: &str) -> ClientConfig {
    let mut config = load_or_create_config::<ClientConfig>(directory, config_file_name)
        .unwrap_or_else(|error| {
            eprintln!(
                "Error loading config: {}. Using default configuration.",
                error.err_msg
            );
            error.data.unwrap_or_else(ClientConfig::default)
        });
    let path = directory.join(config_file_name);
    // The generic loader can write missing fields while returning the original
    // parsed value. Persist normalization of that value, including explicit
    // binding-override markers; native configuration never injects default keys.
    let previous_settings = config.settings.clone();
    config.settings.normalize();
    let requires_migration = previous_settings != config.settings
        || std::fs::read_to_string(&path).is_ok_and(|text| client_config_requires_migration(&text));
    if requires_migration {
        let saved = toml::to_string_pretty(&config)
            .map_err(|error| error.to_string())
            .and_then(|text| std::fs::write(&path, text).map_err(|error| error.to_string()));
        if let Err(error) = saved {
            eprintln!("Error migrating client config: {error}");
        }
    }
    config
}

// Migration is selected from document shape rather than a stored version.
fn client_config_requires_migration(text: &str) -> bool {
    [
        "quic_addr",
        "game_addr",
        "https_addr",
        "game_port",
        "https_port",
        "resource_port",
    ]
    .into_iter()
    .any(|field| {
        text.lines()
            .any(|line| line.trim_start().starts_with(field))
    })
}

/// Serializes and overwrites a configuration file under the executable root.
///
/// Serialization completes before the file is opened. The filesystem write is
/// not transactional: an I/O failure may leave an existing file truncated or
/// partially written.
pub fn save_config<T: Serialize>(config_file_name: &str, config: &T) -> Result<(), String> {
    let content = toml::to_string_pretty(config)
        .map_err(|error| format!("cannot serialize config: {error}"))?;
    return std::fs::write(get_exe_root_path().join(config_file_name), content)
        .map_err(|error| format!("cannot save config: {error}"));
}

/// Configuration shared by client and server processes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CommonConfig {
    pub mod_path: String,
}

impl CommonConfig {
    /// Relative paths use the directory shared by the executables and config files.
    pub fn resolved_mod_path(&self) -> std::path::PathBuf {
        let configured = std::path::PathBuf::from(&self.mod_path);
        if configured.is_absolute() {
            configured
        } else {
            get_exe_root_path().join(configured)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
/// Client endpoint and certificate-verification settings.
pub struct ClientNetworkConfig {
    pub endpoint: EndpointConfig,
    pub ca_verification: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
/// Server endpoint and certificate-generation settings.
pub struct ServerNetworkConfig {
    pub endpoint: EndpointConfig,
    pub certificate_path: String,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
}
#[derive(Clone, Debug, Serialize)]
/// Host and QUIC port of one network endpoint.
pub struct EndpointConfig {
    pub host: String,
    pub quic_port: u16,
}

impl<'de> Deserialize<'de> for EndpointConfig {
    fn deserialize<Deserializer>(deserializer: Deserializer) -> Result<Self, Deserializer::Error>
    where
        Deserializer: serde::Deserializer<'de>,
    {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Input {
            host: Option<String>,
            quic_port: Option<u16>,
            game_port: Option<u16>,
            https_port: Option<serde::de::IgnoredAny>,
            resource_port: Option<serde::de::IgnoredAny>,
        }

        let input = Input::deserialize(deserializer)?;
        let _ = (input.https_port, input.resource_port);
        let defaults = Self::default();
        Ok(Self {
            host: input.host.unwrap_or(defaults.host),
            quic_port: input
                .quic_port
                .or(input.game_port)
                .unwrap_or(defaults.quic_port),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
/// PostgreSQL connection and pool-size settings.
pub struct DatabaseConfig {
    pub url: String,
    pub pool_size: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
/// Server simulation frequency and presence radius.
pub struct GameplayConfig {
    pub tick_rate: u32,
    pub presence_radius: f32,
}
