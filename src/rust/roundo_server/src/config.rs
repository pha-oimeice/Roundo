//! Server process configuration. Environment-variable overrides are intentionally
//! unsupported so the categorized TOML files are the single source of truth.

use roundo_user_config::{CommonConfig, ConfigDirectory, ServerConfig};
use std::sync::LazyLock;

pub static COMMON_CONFIG: LazyLock<CommonConfig> =
    LazyLock::new(roundo_user_config::load_common_config);
pub static SERVER_CONFIG: LazyLock<ServerConfig> = LazyLock::new(load_config);

/// Loads all required server categories, failing on missing or invalid files.
pub fn load_config() -> ServerConfig {
    roundo_user_config::load_server_config()
}

/// Records one never-reused instance identity before making it issuable. The
/// configured interval must be reserved to this installation by the host; this
/// local checkpoint is not a cross-installation global allocator. A stale lock
/// fails closed and must be investigated, not automatically stolen.
pub fn reserve_item_identity() -> anyhow::Result<Option<roundo_items::ItemIssuer>> {
    use anyhow::Context;
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
    let directory = ConfigDirectory::executable();
    let mut item_ids = directory
        .load_item_identity()
        .map_err(anyhow::Error::msg)
        .context("read instance-ID checkpoint")?;
    let (next, last) = match (&item_ids.next, &item_ids.last) {
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
    item_ids.next = if next == last {
        None
    } else {
        Some((next + 1).to_string())
    };
    directory
        .save_item_identity(&item_ids)
        .map_err(anyhow::Error::msg)
        .context("record and flush reserved instance ID before issuance")?;
    let range = roundo_items::ItemIdRange::try_new(next, next)?;
    let issuer = roundo_items::ItemIssuer::claim_host_reserved(range)?;
    Ok(Some(issuer))
}
