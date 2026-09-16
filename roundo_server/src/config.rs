//! Server process configuration. Environment-variable overrides are intentionally
//! unsupported so the generated TOML files are the single source of truth.

use roundo_user_config::{CommonConfig, ServerConfig};
use std::sync::LazyLock;

const CONFIG_FILE_NAME: &str = "roundo-server-config.toml";
pub static COMMON_CONFIG: LazyLock<CommonConfig> =
    LazyLock::new(roundo_user_config::load_common_config);
pub static SERVER_CONFIG: LazyLock<ServerConfig> = LazyLock::new(load_config);

/// Loads server configuration and persists any required migration.
pub fn load_config() -> ServerConfig {
    let config = roundo_user_config::load_config(CONFIG_FILE_NAME);
    let (config, migrated) = migrate_legacy_database_url(config);
    if migrated {
        if let Err(error) = roundo_user_config::save_config(CONFIG_FILE_NAME, &config) {
            eprintln!("Error migrating server config: {error}");
        }
    }
    config
}

// Replaces only the historical placeholder URL, preserving explicit values.
fn migrate_legacy_database_url(mut config: ServerConfig) -> (ServerConfig, bool) {
    const LEGACY_PLACEHOLDER: &str = "postgres://user:password@localhost/dbname";
    if config.database.url == LEGACY_PLACEHOLDER {
        config.database.url = roundo_user_config::DatabaseConfig::default().url;
        (config, true)
    } else {
        (config, false)
    }
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_database_url;
    use roundo_user_config::{CommonConfig, DatabaseConfig, ServerConfig};

    #[test]
    fn missing_server_parameters_use_defaults() {
        let config: ServerConfig = toml::from_str("[network]").unwrap();
        assert!(!config.dev_mode);
        assert_eq!(
            config.database.url,
            "postgres://postgres:12345678@localhost/roundo"
        );
    }

    #[test]
    fn migrates_only_the_legacy_database_placeholder() {
        let legacy = ServerConfig {
            database: DatabaseConfig {
                url: "postgres://user:password@localhost/dbname".to_string(),
                pool_size: 10,
            },
            ..Default::default()
        };
        let (legacy, migrated) = migrate_legacy_database_url(legacy);
        assert!(migrated);
        assert_eq!(
            legacy.database.url,
            "postgres://postgres:12345678@localhost/roundo"
        );

        let custom_url = "postgres://custom:secret@db.example/custom";
        let custom = ServerConfig {
            database: DatabaseConfig {
                url: custom_url.to_string(),
                pool_size: 10,
            },
            ..Default::default()
        };
        let (custom, migrated) = migrate_legacy_database_url(custom);
        assert!(!migrated);
        assert_eq!(custom.database.url, custom_url);
    }

    #[test]
    fn common_config_contains_the_mod_path() {
        let config: CommonConfig = toml::from_str("mod_path = 'custom-mods'").unwrap();
        assert_eq!(config.mod_path, "custom-mods");
    }
}
