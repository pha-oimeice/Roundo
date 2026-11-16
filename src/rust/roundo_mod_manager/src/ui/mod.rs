//! UI prototype schemas and loader-bound sources, never UI runtime instances.
//! Mod initialization parses registry metadata. AssetManager reads/validates page
//! and font bytes only on an explicit asset load. Rendering, navigation and action
//! execution remain in roundo_ui; consumers materialize their own instance data.

pub mod expression;
pub mod layout;
pub mod schema;

use crate::{
    AssetDefinition, AssetError, AssetKey, AssetKind, AssetLoadContext, AssetName, AssetPrototype,
    ModTypeInput, ResourceCandidate,
    asset_manager::catalogue::AssetLoader,
    resolve_candidates,
    source::{BoundFile, SourceDirectory},
    valid_resource_local_name,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const MENU_ROOT_SLOT: &str = "roundo.menu-root";
pub const IN_GAME_ROOT_SLOT: &str = "roundo.in-game-root";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ClientInteractionMode {
    Ui,
    InGame,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum UiWorldVisibility {
    Hidden,
    Visible,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PresentationMode {
    Exclusive,
    Concurrent,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UiLayout {
    Fullscreen,
    Windowed { width: u32, height: u32 },
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Palette {
    Dark,
}

/// Authored theme values; source binding is private and no identity is repeated.
#[derive(Debug)]
pub struct Theme {
    pub palette: Palette,
    pub font_size: f32,
    pub debug_receipt_timeout_secs: f32,
}

/// Asset metadata. Name/owner/type are in its catalogue key, not duplicated here.
#[derive(Debug)]
pub struct UiDefinition {
    page_source: Arc<BoundFile>,
    font_source: Arc<BoundFile>,
    pub interaction_mode: ClientInteractionMode,
    pub world_visibility: UiWorldVisibility,
    pub max_instances: u32,
    pub lifecycle_independent: bool,
    pub presentation: PresentationMode,
    pub layout: UiLayout,
    pub background_color: [u8; 4],
    pub imports: BTreeMap<String, String>,
}

/// Immutable loaded template. It contains no model state, focus, ECS entity or
/// running task. Instances may copy authored initial values when materialized.
#[derive(Debug)]
pub struct UiPrototype {
    pub page: schema::Page,
    font: Vec<u8>,
}
impl UiPrototype {
    pub fn font_bytes(&self) -> &[u8] {
        &self.font
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UiAssetError(pub String);

impl AssetLoader for UiDefinition {
    fn load(&self, context: &mut AssetLoadContext<'_>) -> Result<Arc<AssetPrototype>, AssetError> {
        let loaded = (|| {
            let bytes = self.page_source.read()?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|error| format!("{}: {error}", self.page_source.path().display()))?;
            let page: schema::Page = toml::from_str(text)
                .map_err(|error| format!("{}: {error}", self.page_source.path().display()))?;
            page.validate()
                .map_err(|error| format!("{}: {error}", self.page_source.path().display()))?;
            let font = self.font_source.read()?;
            Ok::<_, String>(UiPrototype { page, font })
        })()
        .map_err(|message| AssetError::LoadFailed {
            asset: context.key().clone(),
            source: Arc::new(UiAssetError(message)),
        })?;
        Ok(Arc::new(AssetPrototype::Ui(loaded)))
    }
}

/// Shared metadata directory, not a second owner of loaded templates.
#[derive(Clone, Debug)]
pub struct UiCatalogue {
    definitions: BTreeMap<String, (AssetKey, Arc<UiDefinition>)>,
    slots: BTreeMap<String, String>,
    pub theme: Arc<Theme>,
}
impl crate::ModPrototype for UiCatalogue {}

impl UiCatalogue {
    /// Rejects pairing this catalogue with another initialization's manager,
    /// even when both namespaces happen to contain the same business names.
    pub fn validate_manager(&self, assets: &crate::AssetManager) -> Result<(), String> {
        for (name, (key, expected)) in &self.definitions {
            match assets.definition(key) {
                Some(AssetDefinition::Ui(actual)) if Arc::ptr_eq(expected, actual) => {}
                _ => {
                    return Err(format!(
                        "UI catalogue {name} is not bound to this Mod initialization"
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn definition(&self, name: &str) -> Option<&UiDefinition> {
        self.definitions
            .get(name)
            .map(|(_, definition)| definition.as_ref())
    }
    pub fn key(&self, name: &str) -> Option<&AssetKey> {
        self.definitions.get(name).map(|(key, _)| key)
    }
    pub fn slot(&self, slot: &str) -> Option<&str> {
        self.slots.get(slot).map(String::as_str)
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.definitions.keys().map(String::as_str)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    theme: Option<ThemeDeclaration>,
    #[serde(default)]
    slots: BTreeMap<String, String>,
    #[serde(default, rename = "resource")]
    resources: Vec<Declaration>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeDeclaration {
    palette: Palette,
    font: String,
    font_size: f32,
    debug_receipt_timeout_secs: f32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    name: String,
    entry: String,
    interaction_mode: ClientInteractionMode,
    world_visibility: UiWorldVisibility,
    max_instances: u32,
    lifecycle_independent: bool,
    presentation: PresentationMode,
    layout: UiLayout,
    background_color: [u8; 4],
    #[serde(default)]
    imports: BTreeMap<String, String>,
}

pub(crate) fn prepare(
    input: &ModTypeInput,
    directory: &Arc<SourceDirectory>,
) -> Result<(UiCatalogue, Vec<(AssetKey, Arc<AssetDefinition>)>), String> {
    let mut pending = BTreeMap::new();
    let mut candidates: BTreeMap<String, Vec<ResourceCandidate<String>>> = BTreeMap::new();
    let mut themes = Vec::new();
    for config in input.configs() {
        let installed = input
            .mods()
            .mod_by_id(&config.owner)
            .expect("loader-owned configuration");
        let manifest: RegistryFile = config.decode()?;
        if let Some(theme) = manifest.theme {
            if !theme.font_size.is_finite()
                || theme.font_size <= 0.0
                || !theme.debug_receipt_timeout_secs.is_finite()
                || theme.debug_receipt_timeout_secs <= 0.0
            {
                return Err(format!(
                    "{}: font size and receipt timeout must be positive finite numbers",
                    config.path.display()
                ));
            }
            let font = directory.bind_ui(&installed.id, &theme.font, false)?;
            themes.push(ResourceCandidate {
                owner: installed.id.clone(),
                priority: installed.override_priority,
                value: (
                    Arc::new(Theme {
                        palette: theme.palette,
                        font_size: theme.font_size,
                        debug_receipt_timeout_secs: theme.debug_receipt_timeout_secs,
                    }),
                    font,
                ),
            });
        }
        let mut local = BTreeSet::new();
        for declaration in manifest.resources {
            if !valid_resource_local_name(&declaration.name)
                || !local.insert(declaration.name.clone())
            {
                return Err(format!(
                    "{}: invalid/duplicate UI definition {}",
                    installed.id, declaration.name
                ));
            }
            if declaration.max_instances == 0
                || matches!(
                    declaration.layout,
                    UiLayout::Windowed { width: 0, .. } | UiLayout::Windowed { height: 0, .. }
                )
            {
                return Err(format!(
                    "{}: invalid instance limit/window size",
                    declaration.name
                ));
            }
            if declaration
                .imports
                .keys()
                .any(|handle| !valid_resource_local_name(handle))
            {
                return Err(format!("{}: invalid UI import handle", declaration.name));
            }
            let page = directory.bind_ui(&installed.id, &declaration.entry, true)?;
            let name = AssetName::new(installed.id.clone(), declaration.name.clone())
                .map_err(|error| error.to_string())?;
            pending.insert(
                name.to_string(),
                (AssetKey::new(AssetKind::Ui, name), declaration, page),
            );
        }
        for (slot, reference) in manifest.slots {
            if slot.is_empty() {
                return Err("empty UI slot".into());
            }
            let name = input
                .mods()
                .resolve_resource_reference(&installed.id, &local, &reference)
                .map_err(|error| error.to_string())?
                .to_string();
            candidates.entry(slot).or_default().push(ResourceCandidate {
                owner: installed.id.clone(),
                priority: installed.override_priority,
                value: name,
            });
        }
    }
    let mut slots = BTreeMap::new();
    for (slot, choices) in candidates {
        let selected = resolve_candidates(choices).expect("nonempty slot candidates");
        if !pending.contains_key(&selected.value) {
            return Err(format!(
                "UI slot {slot}: unknown resource {}",
                selected.value
            ));
        }
        slots.insert(slot, selected.value);
    }
    for required in [MENU_ROOT_SLOT, IN_GAME_ROOT_SLOT] {
        if !slots.contains_key(required) {
            return Err(format!("required UI root slot {required} is missing"));
        }
    }
    let (theme, font) = resolve_candidates(themes)
        .ok_or("native UI theme is not configured")?
        .value;
    let mut definitions = BTreeMap::new();
    let mut bindings = Vec::new();
    for (name, (key, declaration, page)) in pending {
        let mut imports = BTreeMap::new();
        for (handle, slot) in declaration.imports {
            imports.insert(
                handle,
                slots
                    .get(&slot)
                    .cloned()
                    .ok_or_else(|| format!("{name}: unknown import slot {slot}"))?,
            );
        }
        let definition = Arc::new(UiDefinition {
            page_source: page,
            font_source: Arc::clone(&font),
            interaction_mode: declaration.interaction_mode,
            world_visibility: declaration.world_visibility,
            max_instances: declaration.max_instances,
            lifecycle_independent: declaration.lifecycle_independent,
            presentation: declaration.presentation,
            layout: declaration.layout,
            background_color: declaration.background_color,
            imports,
        });
        bindings.push((
            key.clone(),
            Arc::new(AssetDefinition::Ui(Arc::clone(&definition))),
        ));
        definitions.insert(name, (key, definition));
    }
    Ok((
        UiCatalogue {
            definitions,
            slots,
            theme,
        },
        bindings,
    ))
}
