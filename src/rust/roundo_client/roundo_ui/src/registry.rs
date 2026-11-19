//! Runtime adapter over ModManager-owned UI prototypes. No directory scanning,
//! configuration parsing or independent template ownership lives in this crate.

pub(crate) use roundo_mod_manager::ui::{
    ClientInteractionMode, MENU_ROOT_SLOT, Palette, PresentationMode, UiLayout,
};
use roundo_mod_manager::{
    AssetAccess, AssetPrototype,
    ui::{Theme, UiCatalogue, UiDefinition, schema::Page},
};
use std::{collections::BTreeMap, sync::Arc};

/// Materialized data for one UI instance. This is intentionally an owned copy
/// of authored initial values, not a second registered prototype or a live asset.
#[derive(Clone, Debug)]
pub struct UiResource {
    pub page: Page,
    pub interaction_mode: ClientInteractionMode,
    pub max_instances: u32,
    pub lifecycle_independent: bool,
    pub presentation: PresentationMode,
    pub layout: UiLayout,
    pub background_color: [u8; 4],
    pub imports: BTreeMap<String, String>,
}

#[derive(Clone)]
pub(crate) struct UiRegistry {
    catalogue: UiCatalogue,
    assets: AssetAccess,
    pub theme: Arc<Theme>,
}
impl UiRegistry {
    pub fn new(catalogue: UiCatalogue, assets: AssetAccess) -> Self {
        assets
            .with(|manager| catalogue.validate_manager(manager))
            .expect("UI catalogue/Mod manager initialization mismatch");
        let theme = Arc::clone(&catalogue.theme);
        Self {
            catalogue,
            assets,
            theme,
        }
    }
    pub fn resource(&self, name: &str) -> Option<&UiDefinition> {
        self.catalogue.definition(name)
    }
    pub fn slot_name(&self, slot: &str) -> Option<&str> {
        self.catalogue.slot(slot)
    }

    pub fn materialize(&self, name: &str) -> Result<UiResource, String> {
        let definition = self
            .catalogue
            .definition(name)
            .ok_or_else(|| format!("unknown UI prototype {name}"))?;
        let key = self.catalogue.key(name).expect("UI catalogue identity");
        let page = self.assets.with(|assets| {
            let handle = assets.load(key).map_err(|error| error.to_string())?;
            assets
                .read(&handle, |prototype| match prototype {
                    AssetPrototype::Ui(ui) => Ok(ui.page.clone()),
                    _ => Err(format!("{name} is not a UI prototype")),
                })
                .map_err(|error| error.to_string())?
        })?;
        Ok(UiResource {
            page,
            interaction_mode: definition.interaction_mode,
            max_instances: definition.max_instances,
            lifecycle_independent: definition.lifecycle_independent,
            presentation: definition.presentation,
            layout: definition.layout,
            background_color: definition.background_color,
            imports: definition.imports.clone(),
        })
    }

    /// Copies font data into the renderer's independent native Font projection.
    pub fn font(&self) -> Result<Vec<u8>, String> {
        let name = self
            .catalogue
            .slot(MENU_ROOT_SLOT)
            .ok_or("menu UI slot is missing")?;
        let key = self.catalogue.key(name).expect("UI catalogue identity");
        self.assets.with(|assets| {
            let handle = assets.load(key).map_err(|error| error.to_string())?;
            assets
                .read(&handle, |prototype| match prototype {
                    AssetPrototype::Ui(ui) => Ok(ui.font_bytes().to_vec()),
                    _ => Err(format!("{name} is not a UI prototype")),
                })
                .map_err(|error| error.to_string())?
        })
    }
}
