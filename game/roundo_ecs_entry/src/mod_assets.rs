use bevy::asset::AssetPlugin;

const MOD_ASSET_PATH: &str = "mods";
const PROCESSED_MOD_ASSET_PATH: &str = "imported_assets/mods";

pub(super) fn mod_asset_plugin() -> AssetPlugin {
    AssetPlugin {
        file_path: MOD_ASSET_PATH.to_owned(),
        processed_file_path: PROCESSED_MOD_ASSET_PATH.to_owned(),
        ..Default::default()
    }
}
