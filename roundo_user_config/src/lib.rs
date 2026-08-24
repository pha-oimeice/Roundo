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

pub fn save_config<T: Serialize>(config_file_name: &str, config: &T) -> Result<(), String> {
    let content = toml::to_string_pretty(config)
        .map_err(|error| format!("cannot serialize config: {error}"))?;
    std::fs::write(get_exe_root_path().join(config_file_name), content)
        .map_err(|error| format!("cannot save config: {error}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientNetworkConfig {
    pub endpoint: EndpointConfig,
    pub ca_verification: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerNetworkConfig {
    pub endpoint: EndpointConfig,
    pub certificate_path: String,
    pub server_alternative_names: Vec<String>,
    pub generate_self_signed_certificate: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EndpointConfig {
    pub host: String,
    #[serde(alias = "game_port")]
    pub quic_port: u16,
    pub https_port: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
    pub pool_size: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GameplayConfig {
    pub tick_rate: u32,
    pub presence_radius: f32,
}
