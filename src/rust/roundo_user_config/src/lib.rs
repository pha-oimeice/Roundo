//! Persistent client, server, network, database, and gameplay configuration.

mod client_settings;
mod config_directory;
mod config_document;
mod my_impls;
mod process_config;

pub use client_settings::*;
pub use config_directory::ConfigDirectory;
pub use process_config::*;

use roundo_toolbox::fs::get_exe_root_path;
use serde::{Deserialize, Serialize};

/// Loads required shared resources from the executable's `config/common` directory.
pub fn load_common_config() -> CommonConfig {
    ConfigDirectory::executable()
        .load_common()
        .unwrap_or_else(|error| panic!("{error}"))
}

/// Loads required client categories from the executable's `config/client` directory.
pub fn load_client_config() -> ClientConfig {
    ConfigDirectory::executable()
        .load_client()
        .unwrap_or_else(|error| panic!("{error}"))
}

/// Loads required server categories without inventing missing configuration.
pub fn load_server_config() -> ServerConfig {
    ConfigDirectory::executable()
        .load_server()
        .unwrap_or_else(|error| panic!("{error}"))
}

/// Configuration shared by client and server processes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommonConfig {
    pub mod_path: String,
}

impl CommonConfig {
    /// Relative resource paths use the executable directory, not `config/common`.
    pub fn resolved_mod_path(&self) -> std::path::PathBuf {
        let configured = std::path::PathBuf::from(&self.mod_path);
        if configured.is_absolute() {
            configured
        } else {
            get_exe_root_path().join(configured)
        }
    }
}

/// Client endpoint and certificate-verification settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientNetworkConfig {
    pub endpoint: EndpointConfig,
    pub ca_verification: bool,
}

/// Server endpoint and certificate-generation settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerNetworkConfig {
    pub endpoint: EndpointConfig,
    pub certificate_path: String,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
}

/// Host and QUIC port of one network endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EndpointConfig {
    pub host: String,
    pub quic_port: u16,
}

/// PostgreSQL connection and pool-size settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub pool_size: u32,
}

/// Server simulation frequency and presence radius.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameplayConfig {
    pub tick_rate: u32,
    pub presence_radius: f32,
}
