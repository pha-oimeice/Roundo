use crate::{
    ClientNetworkConfig, DatabaseConfig, EndpointConfig, GameplayConfig, ServerNetworkConfig,
};
use roundo_toolbox::fs::get_exe_root_path;
impl Default for ClientNetworkConfig {
    fn default() -> Self {
        ClientNetworkConfig {
            endpoint: EndpointConfig::default(),
            ca_verification: false,
        }
    }
}
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
impl Default for EndpointConfig {
    fn default() -> Self {
        EndpointConfig {
            host: "127.0.0.1".to_string(),
            game_port: 12358,
            https_port: 35813,
        }
    }
}
impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            url: "postgres://user:password@localhost/dbname".to_string(),
            pool_size: 10,
        }
    }
}
impl Default for GameplayConfig {
    fn default() -> Self {
        GameplayConfig {
            tick_rate: 20,
            presence_radius: 128.0,
        }
    }
}
