//! Mod Resource 的 Web UI Registry adapter。
//!
//! 此 module 拥有不可变 UI Registry、路径验证与 UI Registry Slot 裁决；
//! 它不依赖 Wry 或 Bevy runtime。

use roundo_mod_loader::{LoadedMods, ModId, ResourceCandidate, parse_mod_id, resolve_candidates};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Component, Path, PathBuf},
};

pub const DISCONNECTED_ROOT_SLOT: &str = "roundo.disconnected-root";
pub const CONNECTED_ROOT_SLOT: &str = "roundo.connected-root";
pub(crate) const MAX_PREPARED_UI_CANDIDATES: usize = 8;
pub(crate) const MAX_PREPARED_COMMANDS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientInteractionMode {
    WebUi,
    InGame,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiWorldVisibility {
    Hidden,
    Visible,
}

#[derive(Clone, Debug)]
pub struct UiResource {
    pub name: String,
    pub owner: ModId,
    pub project_root: PathBuf,
    pub entry: PathBuf,
    pub entry_path: String,
    pub interaction_mode: ClientInteractionMode,
    pub world_visibility: UiWorldVisibility,
    pub max_instances: u32,
    pub lifecycle_independent: bool,
    pub presentation: PresentationMode,
    pub layout: UiLayout,
    /// Definition names that are likely to be opened after this one. The
    /// platform adapter may prepare them physically, but they do not become
    /// lifecycle instances until a real `ui.open` claims them.
    pub prefetch: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct UiRegistry {
    pub(crate) resources: BTreeMap<String, UiResource>,
    pub(crate) slots: BTreeMap<String, String>,
}
impl UiRegistry {
    pub fn resource(&self, name: &str) -> Option<&UiResource> {
        self.resources.get(name)
    }
    pub fn slot(&self, name: &str) -> Option<&UiResource> {
        self.slots
            .get(name)
            .and_then(|resource| self.resource(resource))
    }
    pub fn slots(&self) -> impl Iterator<Item = (&str, &str)> {
        self.slots
            .iter()
            .map(|(slot, resource)| (slot.as_str(), resource.as_str()))
    }

    /// Returns an asset only when it remains below the registered project root.
    /// This is also the sole filesystem access path used by the Wry protocol.
    pub fn read_asset(
        &self,
        resource: &str,
        relative: &str,
    ) -> Result<(Vec<u8>, &'static str), UiRegistryError> {
        let resource = self
            .resource(resource)
            .ok_or_else(|| UiRegistryError::UnknownResource(resource.into()))?;
        let path = checked_file(&resource.project_root, relative)?;
        let mime = match path.extension().and_then(|extension| extension.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("woff2") => "font/woff2",
            Some("ttf") => "font/ttf",
            _ => "application/octet-stream",
        };
        fs::read(path)
            .map(|bytes| (bytes, mime))
            .map_err(|error| UiRegistryError::Io(resource.project_root.clone(), error))
    }

    /// Resolves a custom-protocol request. Once a document has a bound source
    /// Definition, frames and assets may only stay inside that Definition.
    /// Cross-Definition documents must be created through `ui.open`.
    pub fn read_protocol_asset(
        &self,
        source_resource: Option<&str>,
        authority: &str,
        path: &str,
    ) -> Result<(Vec<u8>, &'static str), UiRegistryError> {
        let (name, relative, is_slot) = protocol_target(authority, path)?;
        let resource = if is_slot {
            self.slot(&name)
                .ok_or_else(|| UiRegistryError::UnknownSlot(name.clone()))?
        } else {
            self.resource(&name)
                .ok_or_else(|| UiRegistryError::UnknownResource(name.clone()))?
        };
        if let Some(source) = source_resource {
            if source != resource.name {
                return Err(UiRegistryError::InvalidReference(format!(
                    "cross-definition frame or asset from `{source}` to `{}`",
                    resource.name
                )));
            }
        }
        let relative = if relative.is_empty() {
            &resource.entry_path
        } else {
            relative.as_str()
        };
        self.read_asset(&resource.name, relative)
    }

    pub fn load(mods: &LoadedMods) -> Result<Self, UiRegistryError> {
        let mut resources = BTreeMap::new();
        let mut candidates = Vec::new();
        for loaded in mods.iter() {
            // Web UI is an optional resource type. Mods without its registry
            // do not contribute Web UI resources.
            let path = loaded.root.join("assets/webui/registry.toml");
            if !path.is_file() {
                continue;
            }
            let source = fs::read_to_string(&path)
                .map_err(|error| UiRegistryError::Io(path.clone(), error))?;
            let manifest: RegistryFile = toml::from_str(&source).map_err(|error| {
                UiRegistryError::InvalidRegistry(path.clone(), error.to_string())
            })?;
            let web_root = canonical_directory(&loaded.root.join("assets/webui"))?;
            let closure = mods
                .dependency_closure(&loaded.id)
                .map_err(UiRegistryError::ModLoader)?;
            let mut local = BTreeSet::new();
            let mut local_prefetch = Vec::new();
            for definition in manifest.ui {
                if !valid_local_name(&definition.name) || !local.insert(definition.name.clone()) {
                    return Err(UiRegistryError::InvalidLocalName {
                        owner: loaded.id.clone(),
                        name: definition.name,
                    });
                }
                if definition.max_instances == 0 {
                    return Err(UiRegistryError::InvalidUiDefinition {
                        owner: loaded.id.clone(),
                        name: definition.name,
                        message: "max_instances must be a positive integer".into(),
                    });
                }
                let name = format!("{}.{}", loaded.id, definition.name);
                local_prefetch.push((name.clone(), definition.prefetch.clone()));
                let project_root = checked_directory(&web_root, &definition.project)?;
                let entry = checked_file(&project_root, &definition.entry)?;
                let resource = UiResource {
                    name: name.clone(),
                    owner: loaded.id.clone(),
                    project_root,
                    entry,
                    entry_path: definition.entry.clone(),
                    interaction_mode: definition.interaction_mode.into(),
                    world_visibility: definition.world_visibility.into(),
                    max_instances: definition.max_instances,
                    lifecycle_independent: definition.lifecycle_independent,
                    presentation: definition.presentation.into(),
                    layout: definition
                        .layout
                        .into_layout(definition.initial_width, definition.initial_height)
                        .map_err(|message| UiRegistryError::InvalidUiDefinition {
                            owner: loaded.id.clone(),
                            name: definition.name.clone(),
                            message,
                        })?,
                    prefetch: Vec::new(),
                };
                if resources.insert(name, resource).is_some() {
                    unreachable!("full names include unique Mod ID and local name")
                }
            }
            let resolve =
                |reference: &str| resolve_reference(reference, &loaded.id, &closure, &local);
            for (source, targets) in local_prefetch {
                let resolved = targets
                    .iter()
                    .map(|target| resolve(target))
                    .collect::<Result<Vec<_>, _>>()?;
                resources
                    .get_mut(&source)
                    .expect("prefetch source was just registered")
                    .prefetch = resolved;
            }
            for (slot, reference) in manifest.slots {
                if slot.is_empty() {
                    return Err(UiRegistryError::InvalidSlot(slot));
                }
                candidates.push((
                    slot,
                    ResourceCandidate {
                        owner: loaded.id.clone(),
                        priority: loaded.load_priority,
                        value: resolve(&reference)?,
                    },
                ));
            }
        }
        for resource in resources.values() {
            for target in &resource.prefetch {
                if !resources.contains_key(target) {
                    return Err(UiRegistryError::UnknownResource(target.clone()));
                }
            }
        }
        let mut grouped: BTreeMap<String, Vec<ResourceCandidate<String>>> = BTreeMap::new();
        for (slot, candidate) in candidates {
            grouped.entry(slot).or_default().push(candidate);
        }
        let mut slots = BTreeMap::new();
        for (slot, candidates) in grouped {
            let selected = resolve_candidates(candidates).expect("a group contains one candidate");
            if !resources.contains_key(&selected.value) {
                return Err(UiRegistryError::UnknownResource(selected.value));
            }
            slots.insert(slot, selected.value);
        }
        Ok(Self { resources, slots })
    }
}

fn protocol_target(authority: &str, path: &str) -> Result<(String, String, bool), UiRegistryError> {
    let mut segments = path.trim_start_matches('/').splitn(2, '/');
    let name = segments.next().unwrap_or_default();
    if name.is_empty() {
        return Err(UiRegistryError::InvalidReference(path.into()));
    }
    let relative = segments.next().unwrap_or_default().into();
    match authority {
        "slot" => Ok((name.into(), relative, true)),
        "resource" => Ok((name.into(), relative, false)),
        _ => Err(UiRegistryError::InvalidReference(format!(
            "unsupported roundo-ui authority `{authority}`"
        ))),
    }
}

pub(crate) fn resolve_reference(
    reference: &str,
    owner: &ModId,
    closure: &BTreeSet<ModId>,
    local: &BTreeSet<String>,
) -> Result<String, UiRegistryError> {
    if valid_local_name(reference) {
        if !local.contains(reference) {
            return Err(UiRegistryError::UnknownLocalResource {
                owner: owner.clone(),
                name: reference.into(),
            });
        }
        return Ok(format!("{owner}.{reference}"));
    }
    let mut parts = reference.split('.');
    let author = parts.next().unwrap_or_default();
    let mod_name = parts.next().unwrap_or_default();
    let local_name = parts.next().unwrap_or_default();
    if parts.next().is_some() || !valid_local_name(local_name) {
        return Err(UiRegistryError::InvalidReference(reference.into()));
    }
    let reference_owner =
        parse_mod_id(&format!("{author}.{mod_name}")).map_err(UiRegistryError::ModLoader)?;
    if !closure.contains(&reference_owner) {
        return Err(UiRegistryError::UndeclaredDependency {
            owner: owner.clone(),
            reference: reference.into(),
        });
    }
    Ok(reference.into())
}

#[derive(Deserialize)]
pub(crate) struct RegistryFile {
    #[serde(default)]
    ui: Vec<RegistryUi>,
    #[serde(default)]
    slots: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct RegistryUi {
    name: String,
    project: String,
    entry: String,
    interaction_mode: InteractionMode,
    world_visibility: WorldVisibility,
    max_instances: u32,
    #[serde(default)]
    lifecycle_independent: bool,
    #[serde(default)]
    presentation: PresentationMode,
    #[serde(default)]
    layout: LayoutRegistration,
    initial_width: Option<u32>,
    initial_height: Option<u32>,
    #[serde(default)]
    prefetch: Vec<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresentationMode {
    Exclusive,
    Concurrent,
}
impl Default for PresentationMode {
    fn default() -> Self {
        Self::Exclusive
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLayout {
    Fullscreen,
    Windowed {
        initial_width: u32,
        initial_height: u32,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LayoutRegistration {
    Fullscreen,
    Windowed,
}
impl Default for LayoutRegistration {
    fn default() -> Self {
        Self::Fullscreen
    }
}
impl LayoutRegistration {
    pub(crate) fn into_layout(
        self,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<UiLayout, String> {
        match self {
            Self::Fullscreen if width.is_none() && height.is_none() => Ok(UiLayout::Fullscreen),
            Self::Fullscreen => {
                Err("initial_width and initial_height are only valid for windowed UI".into())
            }
            Self::Windowed => match (width, height) {
                (Some(initial_width), Some(initial_height))
                    if initial_width > 0 && initial_height > 0 =>
                {
                    Ok(UiLayout::Windowed {
                        initial_width,
                        initial_height,
                    })
                }
                _ => Err("windowed UI requires positive initial_width and initial_height".into()),
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum InteractionMode {
    WebUi,
    InGame,
}
impl From<InteractionMode> for ClientInteractionMode {
    fn from(value: InteractionMode) -> Self {
        match value {
            InteractionMode::WebUi => Self::WebUi,
            InteractionMode::InGame => Self::InGame,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum WorldVisibility {
    Hidden,
    Visible,
}
impl From<WorldVisibility> for UiWorldVisibility {
    fn from(value: WorldVisibility) -> Self {
        match value {
            WorldVisibility::Hidden => Self::Hidden,
            WorldVisibility::Visible => Self::Visible,
        }
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, UiRegistryError> {
    let canonical =
        fs::canonicalize(path).map_err(|error| UiRegistryError::Io(path.into(), error))?;
    if !canonical.is_dir() {
        return Err(UiRegistryError::NotDirectory(canonical));
    }
    Ok(canonical)
}
fn checked_directory(root: &Path, relative: &str) -> Result<PathBuf, UiRegistryError> {
    checked_path(root, relative, true)
}
pub(crate) fn checked_file(root: &Path, relative: &str) -> Result<PathBuf, UiRegistryError> {
    checked_path(root, relative, false)
}
fn checked_path(root: &Path, relative: &str, directory: bool) -> Result<PathBuf, UiRegistryError> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(UiRegistryError::UnsafePath(relative.into()));
    }
    let canonical = fs::canonicalize(root.join(path))
        .map_err(|error| UiRegistryError::Io(root.join(path), error))?;
    if !canonical.starts_with(root) {
        return Err(UiRegistryError::UnsafePath(relative.into()));
    }
    if canonical.is_dir() != directory {
        return Err(if directory {
            UiRegistryError::NotDirectory(canonical)
        } else {
            UiRegistryError::NotFile(canonical)
        });
    }
    Ok(canonical)
}
fn valid_local_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('.')
        && name
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

#[derive(Debug)]
pub enum UiRegistryError {
    Io(PathBuf, std::io::Error),
    InvalidRegistry(PathBuf, String),
    ModLoader(roundo_mod_loader::LoadError),
    InvalidLocalName {
        owner: ModId,
        name: String,
    },
    InvalidUiDefinition {
        owner: ModId,
        name: String,
        message: String,
    },
    UnknownLocalResource {
        owner: ModId,
        name: String,
    },
    InvalidReference(String),
    UndeclaredDependency {
        owner: ModId,
        reference: String,
    },
    InvalidSlot(String),
    UnknownSlot(String),
    UnknownResource(String),
    UnsafePath(String),
    NotDirectory(PathBuf),
    NotFile(PathBuf),
    WebViewUnavailable,
    Navigation(String),
}
impl fmt::Display for UiRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, error) => write!(f, "cannot access {}: {error}", path.display()),
            Self::InvalidRegistry(path, error) => {
                write!(f, "invalid UI registry {}: {error}", path.display())
            }
            Self::ModLoader(error) => error.fmt(f),
            Self::InvalidLocalName { owner, name } => {
                write!(f, "invalid or duplicate UI resource `{name}` in `{owner}`")
            }
            Self::InvalidUiDefinition {
                owner,
                name,
                message,
            } => {
                write!(f, "invalid UI definition `{name}` in `{owner}`: {message}")
            }
            Self::UnknownLocalResource { owner, name } => {
                write!(f, "unknown UI resource `{name}` in `{owner}`")
            }
            Self::InvalidReference(value) => write!(f, "invalid UI resource reference `{value}`"),
            Self::UndeclaredDependency { owner, reference } => write!(
                f,
                "`{owner}` references UI resource `{reference}` without declaring its dependency"
            ),
            Self::InvalidSlot(slot) => write!(f, "invalid empty UI slot `{slot}`"),
            Self::UnknownSlot(slot) => write!(f, "unknown UI slot `{slot}`"),
            Self::UnknownResource(resource) => write!(f, "unknown UI resource `{resource}`"),
            Self::UnsafePath(path) => write!(f, "unsafe UI project path `{path}`"),
            Self::NotDirectory(path) => write!(f, "{} is not a directory", path.display()),
            Self::NotFile(path) => write!(f, "{} is not a file", path.display()),
            Self::WebViewUnavailable => write!(f, "Web UI navigation is unavailable"),
            Self::Navigation(error) => write!(f, "cannot navigate Web UI: {error}"),
        }
    }
}
impl std::error::Error for UiRegistryError {}
