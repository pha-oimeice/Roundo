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
                address: format!("{}:{}", endpoint.host, endpoint.quic_port),
            }],
            network,
            settings: ClientSettingsConfig::default(),
        }
    }
}

/// Persisted display name and QUIC address for one client-selectable server.
///
/// Deserialization accepts legacy `quic_addr` and ignores legacy `https_addr`,
/// but serialization emits only the current `name` and `address` fields. Other
/// unknown fields are rejected.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ServerEntry {
    /// User-facing label; uniqueness and non-emptiness are not enforced here.
    pub name: String,
    /// Unparsed endpoint text; runtime adapters perform socket validation.
    pub address: String,
}

impl<'de> Deserialize<'de> for ServerEntry {
    fn deserialize<Deserializer>(deserializer: Deserializer) -> Result<Self, Deserializer::Error>
    where
        Deserializer: serde::Deserializer<'de>,
    {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Input {
            name: String,
            address: Option<String>,
            #[serde(alias = "game_addr")]
            quic_addr: Option<String>,
            // Accepted only to migrate configurations written before HTTPS was removed.
            https_addr: Option<serde::de::IgnoredAny>,
        }

        let input = Input::deserialize(deserializer)?;
        let _ = input.https_addr;
        Ok(Self {
            name: input.name,
            address: input.address.or(input.quic_addr).unwrap_or_default(),
        })
    }
}

/// Persistent server process configuration with defaults for omitted fields.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub dev_mode: bool,
    pub network: ServerNetworkConfig,
    pub database: DatabaseConfig,
    pub gameplay: GameplayConfig,
    pub item_ids: ItemIdentityConfig,
    pub block_item: BlockItemConfig,
}

/// Host-owned instance-ID cursor. Decimal strings preserve the complete u128
/// range in TOML. None means unconfigured or exhausted, never a native default ID.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ItemIdentityConfig {
    pub next: Option<String>,
    pub last: Option<String>,
}

/// Initial values for independently composed sources, not an item prototype.
/// Omitting either field omits that permission source from the instance.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlockItemConfig {
    pub placement_reach: Option<f32>,
    pub destruction_reach: Option<f32>,
}
