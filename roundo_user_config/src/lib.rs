mod client_settings;
mod my_default_configurations;
mod my_impls;

pub use client_settings::*;

use roundo_toolbox::fs::get_exe_root_path;
use roundo_toolbox::{load_or_create_config, string_to_socket_addr};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

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
    pub game_port: u16,
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
}
