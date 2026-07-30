//! Although all struct can be serialized, but please use 'Config' only so that only 1 config file is used.

use roundo_toolbox::fs::get_exe_root_path;
use roundo_user_config::ClientNetworkConfig;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

const CONFIG_FILE_NAME: &str = "roundo-client-config.toml";
pub static CLIENT_CONFIG: LazyLock<ClientConfig> = LazyLock::new(|| load_config());

pub fn load_config() -> ClientConfig {
    roundo_user_config::load_config(CONFIG_FILE_NAME)
}

pub fn save_servers(servers: &[ServerEntry]) -> Result<(), String> {
    let mut config = CLIENT_CONFIG.clone();
    config.servers = servers.to_vec();
    let serialized = toml::to_string_pretty(&config)
        .map_err(|error| format!("Failed to serialize client config: {error}"))?;
    std::fs::write(get_exe_root_path().join(CONFIG_FILE_NAME), serialized)
        .map_err(|error| format!("Failed to save client config: {error}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub network: ClientNetworkConfig,
    pub servers: Vec<ServerEntry>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        let network = ClientNetworkConfig::default();
        let endpoint = &network.endpoint;
        Self {
            servers: vec![ServerEntry {
                name: "Local server".to_string(),
                game_addr: format!("{}:{}", endpoint.host, endpoint.game_port),
                https_addr: format!("{}:{}", endpoint.host, endpoint.https_port),
            }],
            network,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerEntry {
    pub name: String,
    pub game_addr: String,
    pub https_addr: String,
}
