use bevy::{
    asset::{AssetApp, AssetPlugin, io::AssetSourceBuilder},
    prelude::{App, default},
};

const COMMON_ASSET_PATH: &str = "assets/common";
const COMMON_PROCESSED_ASSET_PATH: &str = "imported_assets/common";

pub(super) fn register_common_asset_source(app: &mut App) {
    app.register_asset_source(
        "common",
        AssetSourceBuilder::platform_default(COMMON_ASSET_PATH, Some(COMMON_PROCESSED_ASSET_PATH)),
    );
}

#[cfg(feature = "client")]
pub(super) fn client_asset_plugin() -> AssetPlugin {
    exclusive_asset_plugin("assets/client", "imported_assets/client")
}

#[cfg(feature = "server")]
pub(super) fn server_asset_plugin() -> AssetPlugin {
    exclusive_asset_plugin("assets/server", "imported_assets/server")
}

fn exclusive_asset_plugin(file_path: &str, processed_file_path: &str) -> AssetPlugin {
    AssetPlugin {
        file_path: file_path.to_string(),
        processed_file_path: processed_file_path.to_string(),
        ..default()
    }
}
