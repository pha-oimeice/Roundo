use crate::{
    ClientNetworkConfig, ClientSettingsConfig, DatabaseConfig, GameplayConfig, ServerNetworkConfig,
};
use serde::{Deserialize, Serialize};

/// Persistent client process configuration. Runtime adapters stay out of this
/// crate so this data remains usable by every client command adapter.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub dev_mode: bool,
    pub network: ClientNetworkConfig,
    pub servers: Vec<ServerEntry>,
    pub settings: ClientSettingsConfig,
}
impl Default for ClientConfig {
    fn default() -> Self {
        let network = ClientNetworkConfig::default();
        let endpoint = &network.endpoint;
        Self {
            dev_mode: false,
            servers: vec![ServerEntry {
                name: "Local server".into(),
                quic_addr: format!("{}:{}", endpoint.host, endpoint.quic_port),
                https_addr: format!("{}:{}", endpoint.host, endpoint.https_port),
            }],
            network,
            settings: ClientSettingsConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerEntry {
    pub name: String,
    #[serde(alias = "game_addr")]
    pub quic_addr: String,
    pub https_addr: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub dev_mode: bool,
    pub network: ServerNetworkConfig,
    pub database: DatabaseConfig,
    pub gameplay: GameplayConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_entry_rejects_unknown_fields() {
        assert!(
            toml::from_str::<ServerEntry>("name='x'\nquic_addr='a'\nhttps_addr='b'\nextra=1")
                .is_err()
        );
    }

    #[test]
    fn legacy_process_configs_default_dev_mode_to_false() {
        assert!(
            !toml::from_str::<ClientConfig>("[network]")
                .unwrap()
                .dev_mode
        );
        assert!(
            !toml::from_str::<ServerConfig>("[network]")
                .unwrap()
                .dev_mode
        );
    }
}
