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

/// Records one never-reused instance identity before making it issuable. The
/// configured interval must be reserved to this installation by the host; this
/// local checkpoint is not a cross-installation global allocator. A stale lock
/// fails closed and must be investigated, not automatically stolen.
pub fn reserve_block_item_identity() -> anyhow::Result<Option<roundo_items::ItemIssuer>> {
    use anyhow::Context;
    use std::io::Write;
    let root = roundo_toolbox::fs::get_exe_root_path();
    let lock_path = root.join("roundo-item-id.lock");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .context("cannot exclusively reserve an item ID; inspect roundo-item-id.lock")?;
    struct ReservationLock(std::path::PathBuf, Option<std::fs::File>);
    impl Drop for ReservationLock {
        fn drop(&mut self) {
            // Windows requires closing the handle before deleting the lock.
            self.1.take();
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _lock = ReservationLock(lock_path, Some(lock));
    let path = root.join(CONFIG_FILE_NAME);
    // Do not use the generic fallback loader at the allocation boundary.
    let source = std::fs::read_to_string(&path).context("read instance-ID checkpoint")?;
    let mut config: ServerConfig =
        toml::from_str(&source).context("parse instance-ID checkpoint")?;
    let (next, last) = match (&config.item_ids.next, &config.item_ids.last) {
        (None, None) => return Ok(None),
        (None, Some(_)) => anyhow::bail!("configured item ID interval is exhausted"),
        (Some(_), None) => anyhow::bail!("item_ids.last is required with item_ids.next"),
        (Some(next), Some(last)) => (
            next.parse::<u128>()
                .context("item_ids.next must be a decimal u128")?,
            last.parse::<u128>()
                .context("item_ids.last must be a decimal u128")?,
        ),
    };
    anyhow::ensure!(
        next <= last,
        "item ID cursor is outside the host-reserved interval"
    );
    config.item_ids.next = if next == last {
        None
    } else {
        Some((next + 1).to_string())
    };
    let updated = toml::to_string_pretty(&config).context("serialize instance-ID checkpoint")?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .context("open instance-ID checkpoint for reservation")?;
    file.write_all(updated.as_bytes())
        .context("record reserved instance ID")?;
    file.sync_all()
        .context("flush reserved instance ID before issuance")?;
    let range = roundo_items::ItemIdRange::try_new(next, next)?;
    let issuer = roundo_items::ItemIssuer::claim_host_reserved(range)?;
    Ok(Some(issuer))
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
