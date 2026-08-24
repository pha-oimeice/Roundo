//! Although all struct can be serialized, but please use 'Config' only so that only 1 config file is used.

#[cfg(test)]
use roundo_user_config::DatabaseConfig;
use roundo_user_config::ServerConfig;
use std::sync::LazyLock;

const CONFIG_FILE_NAME: &str = "roundo-server-config.toml";
pub static SERVER_CONFIG: LazyLock<ServerConfig> = LazyLock::new(|| load_config());
pub fn load_config() -> ServerConfig {
    let config = roundo_user_config::load_config(CONFIG_FILE_NAME);
    apply_database_url_override(config, std::env::var("DATABASE_URL").ok())
}

fn apply_database_url_override(
    mut config: ServerConfig,
    database_url: Option<String>,
) -> ServerConfig {
    if let Some(database_url) = database_url.filter(|url| !url.trim().is_empty()) {
        config.database.url = database_url;
    }

    config
}

#[cfg(test)]
mod tests {
    use super::{DatabaseConfig, ServerConfig, apply_database_url_override};

    #[test]
    fn missing_dev_mode_defaults_to_false() {
        let config: ServerConfig = toml::from_str("[network]").unwrap();
        assert!(!config.dev_mode);
    }

    #[test]
    fn database_url_environment_overrides_generated_config() {
        let config = ServerConfig {
            database: DatabaseConfig {
                url: "postgres://generated-user:generated-password@localhost/generated-db"
                    .to_string(),
                pool_size: 10,
            },
            ..Default::default()
        };

        let config = apply_database_url_override(
            config,
            Some("postgres://configured-user:configured-password@localhost/roundo".to_string()),
        );

        assert_eq!(
            config.database.url,
            "postgres://configured-user:configured-password@localhost/roundo"
        );
    }
}
