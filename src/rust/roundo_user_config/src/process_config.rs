use crate::{
    ClientNetworkConfig, ClientSettingsConfig, DatabaseConfig, GameplayConfig, ServerNetworkConfig,
};
use serde::{Deserialize, Serialize};

/// In-memory aggregate of the client configuration categories. Runtime adapters
/// stay out of this crate; this aggregate is not a persisted document format.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub dev_mode: bool,
    pub network: ClientNetworkConfig,
    pub servers: Vec<ServerEntry>,
    pub settings: ClientSettingsConfig,
}

/// Persisted display name and QUIC address for one client-selectable server.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerEntry {
    /// User-facing label; uniqueness and non-emptiness are not enforced here.
    pub name: String,
    /// Unparsed endpoint text; runtime adapters perform socket validation.
    pub address: String,
}

/// In-memory aggregate of the server configuration categories.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub dev_mode: bool,
    pub network: ServerNetworkConfig,
    pub database: DatabaseConfig,
    pub gameplay: GameplayConfig,
    pub item_ids: ItemIdentityConfig,
}

/// Host-owned instance-ID cursor. Decimal strings preserve the complete u128
/// range in TOML. None means unconfigured or exhausted, never a native default ID.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemIdentityConfig {
    pub next: Option<String>,
    pub last: Option<String>,
}
