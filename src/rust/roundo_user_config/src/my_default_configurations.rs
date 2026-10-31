//! Conservative defaults for local development and first-run configuration.

use crate::{
    ClientNetworkConfig, CommonConfig, DatabaseConfig, EndpointConfig, GameplayConfig,
    ServerNetworkConfig,
};
use roundo_toolbox::fs::get_exe_root_path;

impl Default for CommonConfig {
    fn default() -> Self {
        Self {
            mod_path: "mods".to_string(),
        }
    }
}

// Client defaults permit local self-signed development endpoints.
impl Default for ClientNetworkConfig {
    fn default() -> Self {
        ClientNetworkConfig {
            endpoint: EndpointConfig::default(),
            ca_verification: false,
        }
    }
}
// Server certificates are generated beside the executable by default.
impl Default for ServerNetworkConfig {
    fn default() -> Self {
        ServerNetworkConfig {
            endpoint: EndpointConfig::default(),
            certificate_path: get_exe_root_path().to_str().unwrap().to_string(),
            server_alternative_names: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "0.0.0.0".to_string(),
            ],
            generate_self_signed_certificate: true,
        }
    }
}
// Loopback avoids exposing a new installation on external interfaces.
impl Default for EndpointConfig {
    fn default() -> Self {
        EndpointConfig {
            host: "127.0.0.1".to_string(),
            quic_port: 12358,
        }
    }
}
// The development database keeps a bounded connection pool.
impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            url: "postgres://postgres:12345678@localhost/roundo".to_string(),
            pool_size: 10,
        }
    }
}
// Gameplay defaults balance simulation frequency and presence scope.
impl Default for GameplayConfig {
    fn default() -> Self {
        GameplayConfig {
            tick_rate: 20,
            presence_radius: 128.0,
        }
    }
}
