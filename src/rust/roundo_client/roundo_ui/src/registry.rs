//! Native UI Resource Type: Mod-owned TOML pages, slot selection and checked assets.

use crate::schema::Page;
use roundo_mod_loader::{
    LoadedMods, ModId, ResourceCandidate, resolve_candidates, valid_resource_local_name,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
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

#[derive(Clone, Debug)]
pub struct UiResource {
    pub name: String,
    pub owner: ModId,
    pub page: Arc<Page>,
    pub interaction_mode: ClientInteractionMode,
    pub world_visibility: UiWorldVisibility,
    pub max_instances: u32,
    pub lifecycle_independent: bool,
    pub presentation: PresentationMode,
    pub layout: UiLayout,
    pub background_color: [u8; 4],
    pub imports: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct UiRegistry {
    pub(crate) resources: BTreeMap<String, UiResource>,
    pub(crate) slots: BTreeMap<String, String>,
    pub(crate) theme: Theme,
    pub(crate) font: Vec<u8>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub palette: Palette,
    pub font: String,
    pub font_size: f32,
    pub debug_receipt_timeout_secs: f32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Palette {
    Dark,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    theme: Option<Theme>,
    #[serde(default)]
    slots: BTreeMap<String, String>,
    #[serde(default, rename = "resource")]
    resources: Vec<Declaration>,
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

impl UiRegistry {
    pub fn resource(&self, name: &str) -> Option<&UiResource> {
        self.resources.get(name)
    }
    pub fn slot(&self, slot: &str) -> Option<&UiResource> {
        self.slots.get(slot).and_then(|name| self.resource(name))
    }
    pub fn resources(&self) -> impl Iterator<Item = &UiResource> {
        self.resources.values()
    }

    pub fn load(mods: &LoadedMods) -> Result<Self, String> {
        let mut resources = BTreeMap::new();
        let mut candidates: BTreeMap<String, Vec<ResourceCandidate<String>>> = BTreeMap::new();
        let mut pending_imports = Vec::new();
        let mut themes = Vec::new();
        for loaded in mods.iter() {
            let path = loaded.root.join("assets/ui/registry.toml");
            if !path
                .try_exists()
                .map_err(|e| format!("{}: {e}", path.display()))?
            {
                continue;
            }
            let manifest: RegistryFile = load_toml(&path)?;
            let root =
                fs::canonicalize(loaded.root.join("assets/ui")).map_err(|e| e.to_string())?;
            if let Some(theme) = manifest.theme {
                if !theme.font_size.is_finite()
                    || theme.font_size <= 0.0
                    || !theme.debug_receipt_timeout_secs.is_finite()
                    || theme.debug_receipt_timeout_secs <= 0.0
                {
                    return Err(format!(
                        "{}: font size and receipt timeout must be positive finite numbers",
                        path.display()
                    ));
                }
                let font =
                    fs::read(checked_file(&root, &theme.font)?).map_err(|e| e.to_string())?;
                themes.push(ResourceCandidate {
                    owner: loaded.id.clone(),
                    priority: loaded.override_priority,
                    value: (theme, font),
                });
            }
            let mut local = BTreeSet::new();
            for definition in manifest.resources {
                if !valid_resource_local_name(&definition.name)
                    || !local.insert(definition.name.clone())
                {
                    return Err(format!(
                        "{}: invalid/duplicate UI definition {}",
                        loaded.id, definition.name
                    ));
                }
                if definition.max_instances == 0
                    || matches!(
                        definition.layout,
                        UiLayout::Windowed { width: 0, .. } | UiLayout::Windowed { height: 0, .. }
                    )
                {
                    return Err(format!(
                        "{}: invalid instance limit or window size",
                        definition.name
                    ));
                }
                if definition
                    .imports
                    .keys()
                    .any(|handle| !valid_resource_local_name(handle))
                {
                    return Err(format!("{}: invalid import handle", definition.name));
                }
                let page_path = checked_file(&root, &definition.entry)?;
                let page: Page = load_toml(&page_path)?;
                page.validate()
                    .map_err(|e| format!("{}: {e}", page_path.display()))?;
                let name = format!("{}.{}", loaded.id, definition.name);
                pending_imports.push((name.clone(), definition.imports));
                resources.insert(
                    name.clone(),
                    UiResource {
                        name,
                        owner: loaded.id.clone(),
                        page: Arc::new(page),
                        interaction_mode: definition.interaction_mode,
                        world_visibility: definition.world_visibility,
                        max_instances: definition.max_instances,
                        lifecycle_independent: definition.lifecycle_independent,
                        presentation: definition.presentation,
                        layout: definition.layout,
                        background_color: definition.background_color,
                        imports: BTreeMap::new(),
                    },
                );
            }
            for (slot, reference) in manifest.slots {
                if slot.is_empty() {
                    return Err("empty UI slot".into());
                }
                let name = mods
                    .resolve_resource_reference(&loaded.id, &local, &reference)
                    .map_err(|e| e.to_string())?
                    .to_string();
                candidates.entry(slot).or_default().push(ResourceCandidate {
                    owner: loaded.id.clone(),
                    priority: loaded.override_priority,
                    value: name,
                });
            }
        }
        let slots = candidates
            .into_iter()
            .map(|(slot, candidates)| {
                let selected = resolve_candidates(candidates).expect("nonempty slot candidates");
                if !resources.contains_key(&selected.value) {
                    return Err(format!(
                        "UI slot {slot}: unknown resource {}",
                        selected.value
                    ));
                }
                Ok((slot, selected.value))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        for (name, declarations) in pending_imports {
            let imports = declarations
                .into_iter()
                .map(|(handle, slot)| {
                    let target = slots
                        .get(&slot)
                        .cloned()
                        .ok_or_else(|| format!("{name}: unknown import slot {slot}"))?;
                    Ok((handle, target))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            resources.get_mut(&name).unwrap().imports = imports;
        }
        let (theme, font) = resolve_candidates(themes)
            .ok_or("native UI theme is not configured")?
            .value;
        for slot in [MENU_ROOT_SLOT, IN_GAME_ROOT_SLOT] {
            if !slots.contains_key(slot) {
                return Err(format!("required native UI root slot {slot} is missing"));
            }
        }
        Ok(Self {
            resources,
            slots,
            theme,
            font,
        })
    }
}

fn load_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
        return Err(format!("UI resource must use .toml: {}", path.display()));
    }
    let source = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&source).map_err(|e| format!("{}: {e}", path.display()))
}
fn checked_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("unsafe UI path {relative}"));
    }
    let path = fs::canonicalize(root.join(path)).map_err(|e| format!("{relative}: {e}"))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(format!(
            "UI path escapes its Mod resource root or is not a file: {relative}"
        ));
    }
    Ok(path)
}
