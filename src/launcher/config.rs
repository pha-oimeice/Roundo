use roundo_infrastructure::user_config::{ClientConfig, load_client_config};

pub(crate) fn load_config() -> ClientConfig {
    load_client_config()
}
