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
    let config = load_config::<ClientConfig>(config_file_name);
    let path = get_exe_root_path().join(config_file_name);
    let requires_migration =
        std::fs::read_to_string(&path).is_ok_and(|text| client_config_requires_migration(&text));
    if requires_migration {
        if let Err(error) = save_config(config_file_name, &config) {
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

#[cfg(test)]
mod tests {
    use super::{ClientConfig, CommonConfig, client_config_requires_migration};

    #[test]
    fn detects_only_legacy_client_network_fields() {
        assert!(client_config_requires_migration(
            "[[servers]]\nquic_addr='localhost:12358'\nhttps_addr='localhost:35813'"
        ));
        assert!(client_config_requires_migration(
            "[network.endpoint]\nhttps_port=35813"
        ));
        assert!(!client_config_requires_migration(
            "[[servers]]\nname='Local'\naddress='localhost:12358'"
        ));
    }

    #[test]
    fn common_config_defines_mod_path_once() {
        let config = toml::from_str::<CommonConfig>("mod_path = 'custom-mods'").unwrap();
        assert_eq!(config.mod_path, "custom-mods");
        assert!(
            toml::to_string(&config)
                .unwrap()
                .contains("mod_path = \"custom-mods\"")
        );
    }

    #[test]
    fn loads_legacy_and_current_endpoint_ports_together() {
        let config = toml::from_str::<ClientConfig>(
            "[network.endpoint]\nhost='127.0.0.1'\ngame_port=35813\nquic_port=12358\nhttps_port=35813\nresource_port=47910",
        )
        .expect("legacy endpoint fields must not invalidate the client config");
        assert_eq!(config.network.endpoint.quic_port, 12358);
    }
}
