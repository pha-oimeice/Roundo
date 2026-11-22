use roundo_infrastructure::user_config::{ClientConfig, load_client_config, load_common_config};
use std::path::PathBuf;

pub(crate) fn load_config() -> ClientConfig {
    load_client_config()
}

pub(crate) fn mod_path() -> PathBuf {
    load_common_config().resolved_mod_path()
}
