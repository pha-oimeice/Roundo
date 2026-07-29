//! Although all struct can be serialized, but please use 'Config' only so that only 1 config file is used.

use roundo_user_config::ClientNetworkConfig;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

const CONFIG_FILE_NAME: &str = "roundo-client-config.toml";
pub static CLIENT_CONFIG: LazyLock<ClientConfig> = LazyLock::new(|| load_config());
pub fn load_config() -> ClientConfig {
    roundo_user_config::load_config(CONFIG_FILE_NAME)
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub network: ClientNetworkConfig,
}
