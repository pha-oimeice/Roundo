//! Host wiring for native UI and ECS snapshots.

use crate::client_data::{
    BINDINGS_DATA, ClientDataSyncState, HUD_DATA, SETTINGS_DATA, sync_subscribed_client_data,
    update_hud_cache,
};
use bevy::prelude::*;
use roundo_infrastructure::mod_manager::{AssetAccess, AssetPrototype, ui::UiCatalogue};
use roundo_ui::UiSystemSet;

pub(crate) struct ClientUiHostPlugin;

impl Plugin for ClientUiHostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientDataSyncState>().add_systems(
            Update,
            sync_subscribed_client_data
                .after(update_hud_cache)
                .after(UiSystemSet::Ingress)
                .before(UiSystemSet::Project),
        );
    }
}

/// Validates references against the sources available in the single-player host.
pub(crate) fn validate_ui_registry(
    catalogue: &UiCatalogue,
    assets: &AssetAccess,
) -> Result<(), String> {
    assets.with(|assets| -> Result<(), String> {
        catalogue.validate_manager(assets)?;
        for name in catalogue.names() {
            let key = catalogue.key(name).expect("UI catalogue identity");
            let handle = assets.load(key).map_err(|error| error.to_string())?;
            assets
                .read(&handle, |prototype| -> Result<(), String> {
                    let AssetPrototype::Ui(ui) = prototype else {
                        return Err(format!("{name} is not a UI prototype"));
                    };
                    for source in ui.page.sources.values() {
                        if ![SETTINGS_DATA, BINDINGS_DATA, HUD_DATA].contains(&source.as_str()) {
                            return Err(format!("{name}: unknown Client Data source `{source}`"));
                        }
                    }
                    if !ui.page.shortcuts.is_empty() {
                        return Err(format!(
                            "{name}: this client does not install a shortcut input provider"
                        ));
                    }
                    Ok(())
                })
                .map_err(|error| error.to_string())??;
        }
        Ok(())
    })
}
