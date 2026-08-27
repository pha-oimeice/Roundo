//! Mod-registered Web UI resource registry.
//!
//! The registry is deliberately separate from Wry: loading and resolving UI
//! resources is deterministic and testable without a desktop/WebView runtime.

use bevy::prelude::{App, IntoScheduleConfigs, Message, Plugin, Resource};
use roundo_mod_loader::{LoadedMods, ModId, ResourceCandidate, parse_mod_id, resolve_candidates};
use roundo_toolbox::request_response_pipe::{
    JsonRequestResponseIo, JsonSubmitError, RequestCall, ResponseSender,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const DISCONNECTED_ROOT_SLOT: &str = "roundo.disconnected-root";
pub const CONNECTED_ROOT_SLOT: &str = "roundo.connected-root";
const MAX_PREPARED_UI_CANDIDATES: usize = 8;
const MAX_PREPARED_COMMANDS: usize = 64;

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
    resources: BTreeMap<String, UiResource>,
    slots: BTreeMap<String, String>,
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

fn resolve_reference(
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct UiInstanceId(u64);
impl UiInstanceId {
    pub fn get(self) -> u64 {
        self.0
    }
    /// Constructs an identity only from host-owned command transport metadata.
    pub fn from_host_id(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiLifecycleState {
    Disconnected,
    Connected,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiCommandSource {
    Host,
    WebView(UiInstanceId),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FocusedUiDeclaration {
    pub instance: UiInstanceId,
    pub interaction_mode: ClientInteractionMode,
    pub world_visibility: UiWorldVisibility,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct UiInstance {
    pub id: UiInstanceId,
    pub definition: String,
    pub current_path: String,
    pub visible: bool,
    pub loaded: bool,
    /// Runtime logical-pixel geometry belongs to this instance, never to the
    /// Mod definition. It therefore survives ownership reparenting.
    pub bounds: UiBounds,
    pub z_order: u64,
    pub pointer_enabled: bool,
}
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UiOpenTarget {
    resource: String,
    path: String,
}
impl UiOpenTarget {
    pub fn resource(&self) -> &str {
        &self.resource
    }
}
#[derive(Clone, Debug)]
struct PendingUiOpen {
    source: UiCommandSource,
    parent: u64,
    root_generation: u64,
    target: UiOpenTarget,
    root_ui: bool,
    replacement_lifecycle: Option<UiLifecycleState>,
    /// A physically prepared candidate is not a logical open until a caller
    /// claims it. Its opener and parent are rebound atomically on claim.
    prefetched: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiRootReplacement {
    Pending(UiInstanceId),
    Completed(Vec<UiInstanceId>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiRootCommit {
    pub instance: UiInstanceId,
    pub destroyed: Vec<UiInstanceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverySurface {
    pub lifecycle: UiLifecycleState,
    pub failed_resource: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    Retry,
    Disconnect,
    Quit,
}

#[derive(Clone, Copy, Debug, Message)]
pub struct RecoveryActionRequest(pub RecoveryAction);

/// Read-only staged-open data for the platform adapter. The adapter cannot
/// mutate the tree or counters; it may only create/discard external resources
/// and report completion through `commit_open` / `fail_open`.
#[derive(Clone, Debug)]
pub struct PendingUiDescriptor {
    pub id: u64,
    pub target: UiOpenTarget,
    pub root_generation: u64,
    pub prefetched: bool,
}
#[derive(Clone, Debug)]
struct UiAdapterRecord {
    loaded: bool,
    endpoint_enabled: bool,
    /// Stable adapter identity lets platform adapters retain a survivor's DOM
    /// and HWND while the pure tree changes its parent.
    identity: u64,
}

/// Host-owned lifecycle authority. Root anchors are never UI instances.
#[derive(Resource, Debug)]
pub struct UiLifecycleManager {
    pub registry: UiRegistry,
    lifecycle_state: UiLifecycleState,
    root_generation: u64,
    tree: roundo_lifecycle::lifecycle_tree::AnchorTree<u64, Option<String>>,
    instances: BTreeMap<UiInstanceId, UiInstance>,
    live_counts: BTreeMap<String, u32>,
    pending: BTreeMap<u64, PendingUiOpen>,
    adapters: BTreeMap<UiInstanceId, UiAdapterRecord>,
    focus_history: Vec<UiInstanceId>,
    client_bounds: UiBounds,
    next_z_order: u64,
    next_id: u64,
    recovery: Option<RecoverySurface>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiLifecycleError {
    Registry(String),
    UiInstanceLimit,
    DuplicatePendingOpen,
    StaleUiInstance,
    CannotCloseUiRoot,
    NavigationFailed(String),
    LoadTimeout,
    InternalTree(String),
}
impl fmt::Display for UiLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(v) => write!(f, "invalid UI resource: {v}"),
            Self::UiInstanceLimit => write!(f, "UI definition instance limit reached"),
            Self::DuplicatePendingOpen => {
                write!(f, "the same UI open is already pending from this source")
            }
            Self::StaleUiInstance => write!(f, "stale UI instance"),
            Self::CannotCloseUiRoot => write!(f, "cannot close UI root anchor"),
            Self::NavigationFailed(v) => write!(f, "UI navigation failed: {v}"),
            Self::LoadTimeout => write!(f, "UI load timed out"),
            Self::InternalTree(v) => write!(f, "UI lifecycle invariant failed: {v}"),
        }
    }
}
impl std::error::Error for UiLifecycleError {}

impl UiLifecycleManager {
    pub fn new(registry: UiRegistry) -> Self {
        Self {
            registry,
            lifecycle_state: UiLifecycleState::Disconnected,
            root_generation: 1,
            tree: roundo_lifecycle::lifecycle_tree::AnchorTree::new(0, None),
            instances: BTreeMap::new(),
            live_counts: BTreeMap::new(),
            pending: BTreeMap::new(),
            adapters: BTreeMap::new(),
            focus_history: Vec::new(),
            client_bounds: UiBounds {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            },
            next_z_order: 1,
            next_id: 1,
            recovery: None,
        }
    }
    pub fn lifecycle_state(&self) -> UiLifecycleState {
        self.lifecycle_state
    }
    pub fn instance(&self, id: UiInstanceId) -> Option<&UiInstance> {
        self.instances.get(&id)
    }
    pub fn parent_instance(&self, id: UiInstanceId) -> Option<UiInstanceId> {
        self.tree
            .parent(id.0)
            .filter(|parent| *parent != 0)
            .map(UiInstanceId)
    }
    pub fn instances(&self) -> impl Iterator<Item = &UiInstance> {
        self.instances.values()
    }
    /// Returns physical stacking from back to front. Ownership descendants
    /// always remain above their ancestors; focus z-order only chooses between
    /// sibling subtrees. This prevents a focused fullscreen parent from
    /// visually covering a concurrent windowed child.
    pub fn stacking_order(&self) -> Vec<UiInstanceId> {
        fn collect_subtree_z_orders(
            manager: &UiLifecycleManager,
            node: u64,
            output: &mut BTreeMap<u64, u64>,
        ) -> u64 {
            let mut highest = manager
                .instances
                .get(&UiInstanceId(node))
                .map_or(0, |instance| instance.z_order);
            for child in manager.tree.children(node) {
                highest = highest.max(collect_subtree_z_orders(manager, child, output));
            }
            output.insert(node, highest);
            highest
        }

        fn append_subtree(
            manager: &UiLifecycleManager,
            subtree_z_orders: &BTreeMap<u64, u64>,
            parent: u64,
            output: &mut Vec<UiInstanceId>,
        ) {
            let mut children = manager
                .tree
                .children(parent)
                .filter(|id| manager.instances.contains_key(&UiInstanceId(*id)))
                .map(|id| {
                    (
                        subtree_z_orders.get(&id).copied().unwrap_or_default(),
                        UiInstanceId(id),
                    )
                })
                .collect::<Vec<_>>();
            children.sort_by_key(|(branch_z_order, id)| (*branch_z_order, *id));
            for (_, id) in children {
                output.push(id);
                append_subtree(manager, subtree_z_orders, id.0, output);
            }
        }

        let mut subtree_z_orders = BTreeMap::new();
        collect_subtree_z_orders(self, 0, &mut subtree_z_orders);
        let mut output = Vec::with_capacity(self.instances.len());
        append_subtree(self, &subtree_z_orders, 0, &mut output);
        output
    }
    pub fn current_instance(&self) -> Option<&UiInstance> {
        self.focused_instance()
            .and_then(|id| self.instances.get(&id))
            .or_else(|| self.instances.values().next())
    }
    pub fn current_resource(&self) -> Option<&UiResource> {
        self.current_instance()
            .and_then(|instance| self.registry.resource(&instance.definition))
    }
    pub fn live_count(&self, definition: &str) -> u32 {
        self.live_counts.get(definition).copied().unwrap_or(0)
    }
    pub fn adapter_count(&self) -> usize {
        self.adapters.len()
    }
    pub fn adapter_identity(&self, id: UiInstanceId) -> Option<u64> {
        self.adapters.get(&id).map(|adapter| adapter.identity)
    }
    pub fn set_client_bounds(&mut self, width: u32, height: u32) {
        self.client_bounds.width = width;
        self.client_bounds.height = height;
        let fullscreen = self
            .instances
            .iter()
            .filter_map(|(id, instance)| {
                matches!(
                    self.registry.resource(&instance.definition)?.layout,
                    UiLayout::Fullscreen
                )
                .then_some(*id)
            })
            .collect::<Vec<_>>();
        for id in fullscreen {
            self.instances
                .get_mut(&id)
                .expect("collected live instance")
                .bounds = self.client_bounds;
        }
    }
    pub fn set_bounds(
        &mut self,
        id: UiInstanceId,
        bounds: UiBounds,
    ) -> Result<(), UiLifecycleError> {
        let instance = self
            .instances
            .get_mut(&id)
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        if matches!(
            self.registry
                .resource(&instance.definition)
                .map(|definition| &definition.layout),
            Some(UiLayout::Fullscreen)
        ) {
            return Err(UiLifecycleError::NavigationFailed(
                "fullscreen UI bounds are host-managed".into(),
            ));
        }
        instance.bounds = bounds;
        Ok(())
    }
    pub fn focus(&mut self, id: UiInstanceId) -> Result<(), UiLifecycleError> {
        let interactive = self.instances.get(&id).is_some_and(|instance| {
            instance.visible
                && instance.loaded
                && self
                    .registry
                    .resource(&instance.definition)
                    .is_some_and(|d| d.interaction_mode == ClientInteractionMode::WebUi)
        });
        if !interactive {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        self.focus_history.retain(|candidate| *candidate != id);
        self.focus_history.push(id);
        self.next_z_order += 1;
        self.instances
            .get_mut(&id)
            .expect("validated instance")
            .z_order = self.next_z_order;
        self.recompute_presentation();
        Ok(())
    }
    pub fn clear_focus(&mut self) {
        self.focus_history.clear();
    }
    pub fn interactive_instance_at(&self, x: f32, y: f32) -> Option<UiInstanceId> {
        self.stacking_order().into_iter().rev().find(|id| {
            self.instances.get(id).is_some_and(|instance| {
                instance.visible
                    && instance.loaded
                    && instance.pointer_enabled
                    && x >= instance.bounds.x as f32
                    && y >= instance.bounds.y as f32
                    && x < instance.bounds.x as f32 + instance.bounds.width as f32
                    && y < instance.bounds.y as f32 + instance.bounds.height as f32
            })
        })
    }
    pub fn command_source_is_live(&self, id: UiInstanceId) -> bool {
        self.instances.contains_key(&id)
            && self
                .adapters
                .get(&id)
                .is_some_and(|adapter| adapter.loaded && adapter.endpoint_enabled)
    }
    pub fn focused_instance(&self) -> Option<UiInstanceId> {
        self.focus_history.iter().rev().copied().find(|id| {
            self.instances.get(id).is_some_and(|instance| {
                instance.loaded
                    && instance.visible
                    && self
                        .registry
                        .resource(&instance.definition)
                        .is_some_and(|definition| {
                            definition.interaction_mode == ClientInteractionMode::WebUi
                        })
            })
        })
    }
    pub fn focused_declaration(&self) -> Option<FocusedUiDeclaration> {
        let instance = self.focused_instance()?;
        let definition = self
            .registry
            .resource(&self.instances[&instance].definition)?;
        (definition.interaction_mode == ClientInteractionMode::WebUi).then_some(
            FocusedUiDeclaration {
                instance,
                interaction_mode: definition.interaction_mode,
                world_visibility: definition.world_visibility,
            },
        )
    }
    pub fn resolve_slot_path(
        &self,
        slot: &str,
        path: Option<&str>,
    ) -> Result<UiOpenTarget, UiRegistryError> {
        let name = self
            .registry
            .slots
            .get(slot)
            .ok_or_else(|| UiRegistryError::UnknownSlot(slot.into()))?;
        self.resolve_resource_path(name, path)
    }
    pub fn resolve_resource_path(
        &self,
        resource: &str,
        path: Option<&str>,
    ) -> Result<UiOpenTarget, UiRegistryError> {
        let definition = self
            .registry
            .resource(resource)
            .ok_or_else(|| UiRegistryError::UnknownResource(resource.into()))?;
        let path = path.unwrap_or(&definition.entry_path);
        checked_file(&definition.project_root, path)?;
        Ok(UiOpenTarget {
            resource: resource.into(),
            path: path.into(),
        })
    }
    pub fn open_slot(
        &mut self,
        source: UiCommandSource,
        slot: &str,
        path: Option<&str>,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let target = self
            .resolve_slot_path(slot, path)
            .map_err(|e| UiLifecycleError::Registry(e.to_string()))?;
        self.open(source, target)
    }
    pub fn open_resource(
        &mut self,
        source: UiCommandSource,
        resource: &str,
        path: Option<&str>,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let target = self
            .resolve_resource_path(resource, path)
            .map_err(|e| UiLifecycleError::Registry(e.to_string()))?;
        self.open(source, target)
    }
    pub fn begin_open(
        &mut self,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<u64, UiLifecycleError> {
        self.begin_open_internal(source, target, false, None, false)
    }

    /// Reserves one hidden physical candidate without creating a logical UI
    /// instance or consuming the Definition's live-instance allowance.
    pub fn begin_prefetch(&mut self, target: UiOpenTarget) -> Result<u64, UiLifecycleError> {
        if let Some((id, _)) = self
            .pending
            .iter()
            .find(|(_, pending)| pending.prefetched && pending.target == target)
        {
            return Ok(*id);
        }
        if self.instances.values().any(|instance| {
            instance.definition == target.resource && instance.current_path == target.path
        }) {
            return Err(UiLifecycleError::UiInstanceLimit);
        }
        self.begin_open_internal(UiCommandSource::Host, target, false, None, true)
    }

    /// Converts a prepared candidate into the caller's real Pending UI Open.
    /// The document keeps its future instance identity, while ownership is
    /// assigned only now so speculative work cannot mutate the lifecycle tree.
    pub fn claim_prefetch(
        &mut self,
        source: UiCommandSource,
        target: &UiOpenTarget,
    ) -> Result<Option<u64>, UiLifecycleError> {
        let parent = self.pending_parent(source)?;
        let Some(id) = self.pending.iter().find_map(|(id, pending)| {
            (pending.prefetched && &pending.target == target).then_some(*id)
        }) else {
            return Ok(None);
        };
        let definition = self
            .registry
            .resource(&target.resource)
            .ok_or_else(|| UiLifecycleError::Registry(target.resource.clone()))?;
        if self.live_count(&target.resource) >= definition.max_instances {
            return Err(UiLifecycleError::UiInstanceLimit);
        }
        let pending = self
            .pending
            .get_mut(&id)
            .expect("selected pending candidate");
        pending.source = source;
        pending.parent = parent;
        pending.prefetched = false;
        Ok(Some(id))
    }

    fn pending_parent(&self, source: UiCommandSource) -> Result<u64, UiLifecycleError> {
        match source {
            UiCommandSource::Host => Ok(0),
            UiCommandSource::WebView(id) if self.command_source_is_live(id) => Ok(id.0),
            UiCommandSource::WebView(_) => Err(UiLifecycleError::StaleUiInstance),
        }
    }

    fn begin_open_internal(
        &mut self,
        source: UiCommandSource,
        target: UiOpenTarget,
        root_ui: bool,
        replacement_lifecycle: Option<UiLifecycleState>,
        prefetched: bool,
    ) -> Result<u64, UiLifecycleError> {
        let parent = self.pending_parent(source)?;
        if self.pending.values().any(|pending| {
            pending.source == source
                && pending.target == target
                && pending.root_generation == self.root_generation
        }) {
            return Err(UiLifecycleError::DuplicatePendingOpen);
        }
        let definition = self
            .registry
            .resource(&target.resource)
            .ok_or_else(|| UiLifecycleError::Registry(target.resource.clone()))?;
        if replacement_lifecycle.is_none()
            && self.live_count(&target.resource) >= definition.max_instances
        {
            return Err(UiLifecycleError::UiInstanceLimit);
        }
        let pending_id = self.next_id;
        self.next_id += 1;
        self.pending.insert(
            pending_id,
            PendingUiOpen {
                source,
                parent,
                root_generation: self.root_generation,
                target,
                root_ui,
                replacement_lifecycle,
                prefetched,
            },
        );
        Ok(pending_id)
    }
    pub fn pending_descriptor(&self, pending_id: u64) -> Option<PendingUiDescriptor> {
        self.pending
            .get(&pending_id)
            .map(|pending| PendingUiDescriptor {
                id: pending_id,
                target: pending.target.clone(),
                root_generation: pending.root_generation,
                prefetched: pending.prefetched,
            })
    }
    pub fn pending_descriptors(&self) -> Vec<PendingUiDescriptor> {
        self.pending
            .iter()
            .map(|(id, pending)| PendingUiDescriptor {
                id: *id,
                target: pending.target.clone(),
                root_generation: pending.root_generation,
                prefetched: pending.prefetched,
            })
            .collect()
    }
    pub fn recovery_surface(&self) -> Option<&RecoverySurface> {
        self.recovery.as_ref()
    }
    pub fn retry_recovery(&mut self) -> Result<u64, UiLifecycleError> {
        let recovery = self
            .recovery
            .clone()
            .ok_or_else(|| UiLifecycleError::NavigationFailed("recovery is not active".into()))?;
        let target = self
            .resolve_resource_path(&recovery.failed_resource, None)
            .map_err(|error| UiLifecycleError::Registry(error.to_string()))?;
        let pending = self.begin_root_replacement_open(recovery.lifecycle, target)?;
        self.recovery = None;
        Ok(pending)
    }
    pub fn fail_open(
        &mut self,
        pending_id: u64,
        message: impl Into<String>,
    ) -> Result<(), UiLifecycleError> {
        let pending = self
            .pending
            .remove(&pending_id)
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        if pending.root_ui && pending.root_generation == self.root_generation {
            self.recovery = Some(RecoverySurface {
                lifecycle: pending
                    .replacement_lifecycle
                    .unwrap_or(self.lifecycle_state),
                failed_resource: pending.target.resource,
                message: message.into(),
            });
        }
        Ok(())
    }
    pub fn commit_open(&mut self, pending_id: u64) -> Result<UiInstanceId, UiLifecycleError> {
        let pending = self
            .pending
            .remove(&pending_id)
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        if pending.root_generation != self.root_generation || !self.tree.contains(pending.parent) {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        if let UiCommandSource::WebView(id) = pending.source {
            if !self.command_source_is_live(id) {
                return Err(UiLifecycleError::StaleUiInstance);
            }
        }
        let definition = self
            .registry
            .resource(&pending.target.resource)
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        if self.live_count(&pending.target.resource) >= definition.max_instances {
            return Err(UiLifecycleError::UiInstanceLimit);
        }
        let id = UiInstanceId(pending_id);
        self.tree
            .insert_child(pending.parent, id.0, Some(pending.target.resource.clone()))
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let definition = self
            .registry
            .resource(&pending.target.resource)
            .expect("definition was revalidated");
        let bounds = match definition.layout {
            UiLayout::Fullscreen => self.client_bounds,
            UiLayout::Windowed {
                initial_width,
                initial_height,
            } => UiBounds {
                x: self.client_bounds.x
                    + (self.client_bounds.width.saturating_sub(initial_width) / 2) as i32,
                y: self.client_bounds.y
                    + (self.client_bounds.height.saturating_sub(initial_height) / 2) as i32,
                width: initial_width,
                height: initial_height,
            },
        };
        self.next_z_order += 1;
        self.instances.insert(
            id,
            UiInstance {
                id,
                definition: pending.target.resource.clone(),
                current_path: pending.target.path,
                visible: true,
                loaded: true,
                bounds,
                z_order: self.next_z_order,
                pointer_enabled: definition.interaction_mode == ClientInteractionMode::WebUi,
            },
        );
        *self.live_counts.entry(pending.target.resource).or_default() += 1;
        self.adapters.insert(
            id,
            UiAdapterRecord {
                loaded: true,
                endpoint_enabled: true,
                identity: id.0,
            },
        );
        if definition.interaction_mode == ClientInteractionMode::WebUi {
            self.focus_history.retain(|v| *v != id);
            self.focus_history.push(id);
        }
        if pending.root_ui {
            self.recovery = None;
        }
        self.recompute_presentation();
        Ok(id)
    }

    pub fn pending_root_replacement(&self, pending_id: u64) -> Option<UiLifecycleState> {
        self.pending
            .get(&pending_id)
            .and_then(|pending| pending.replacement_lifecycle)
    }

    pub fn pending_root_replacement_lifecycle(&self) -> Option<UiLifecycleState> {
        self.pending
            .values()
            .find_map(|pending| pending.replacement_lifecycle)
    }

    pub fn cancel_root_replacement(&mut self) -> Vec<u64> {
        let cancelled = self
            .pending
            .iter()
            .filter_map(|(id, pending)| pending.replacement_lifecycle.map(|_| *id))
            .collect::<Vec<_>>();
        for id in &cancelled {
            self.pending.remove(id);
        }
        cancelled
    }

    pub fn commit_root_replacement(
        &mut self,
        pending_id: u64,
    ) -> Result<UiRootCommit, UiLifecycleError> {
        let mut pending = self
            .pending
            .remove(&pending_id)
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        let lifecycle = pending
            .replacement_lifecycle
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        if pending.root_generation != self.root_generation {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        let destroyed = self.replace_root(lifecycle)?;
        pending.root_generation = self.root_generation;
        pending.replacement_lifecycle = None;
        self.pending.insert(pending_id, pending);
        let instance = self.commit_open(pending_id)?;
        Ok(UiRootCommit {
            instance,
            destroyed,
        })
    }

    pub fn open(
        &mut self,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let pending = self.begin_open(source, target)?;
        self.commit_open(pending)
    }
    pub fn navigate_same_definition(
        &mut self,
        source: UiInstanceId,
        path: &str,
    ) -> Result<(), UiLifecycleError> {
        let instance = self
            .instances
            .get(&source)
            .ok_or(UiLifecycleError::StaleUiInstance)?;
        let target = self
            .resolve_resource_path(&instance.definition, Some(path))
            .map_err(|error| UiLifecycleError::Registry(error.to_string()))?;
        self.instances
            .get_mut(&source)
            .expect("source was validated")
            .current_path = target.path;
        Ok(())
    }
    pub fn back(&mut self, source: UiInstanceId) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        if !self.instances.contains_key(&source) {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        let registry = &self.registry;
        let plan = self
            .tree
            .plan_destruction([source.0], false, |_, definition| {
                definition
                    .as_ref()
                    .and_then(|v| registry.resource(v))
                    .is_some_and(|v| v.lifecycle_independent)
            })
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let outcome = self
            .tree
            .commit(plan)
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let destroyed = outcome
            .destroyed
            .into_iter()
            .filter(|v| *v != 0)
            .map(UiInstanceId)
            .collect::<Vec<_>>();
        self.remove_instances(&destroyed);
        Ok(destroyed)
    }
    pub fn replace_root(
        &mut self,
        state: UiLifecycleState,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let plan = self
            .tree
            .plan_destruction([0], true, |_, _| false)
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let outcome = self
            .tree
            .commit(plan)
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let destroyed = outcome
            .destroyed
            .into_iter()
            .filter(|v| *v != 0)
            .map(UiInstanceId)
            .collect::<Vec<_>>();
        self.remove_instances(&destroyed);
        // Prepared candidates have no lifecycle parent until claimed, so a
        // Root replacement must not throw away their already-loaded views.
        // Ordinary Pending UI Opens remain owned by the old Root and die here.
        self.pending.retain(|_, pending| pending.prefetched);
        self.recovery = None;
        self.root_generation += 1;
        for pending in self.pending.values_mut() {
            pending.root_generation = self.root_generation;
            pending.source = UiCommandSource::Host;
            pending.parent = 0;
        }
        self.tree = roundo_lifecycle::lifecycle_tree::AnchorTree::new(0, None);
        self.lifecycle_state = state;
        Ok(destroyed)
    }
    fn configured_root_target_for(
        &self,
        lifecycle: UiLifecycleState,
    ) -> Result<Option<UiOpenTarget>, UiLifecycleError> {
        let slot = match lifecycle {
            UiLifecycleState::Disconnected => DISCONNECTED_ROOT_SLOT,
            UiLifecycleState::Connected => CONNECTED_ROOT_SLOT,
        };
        if self.registry.slot(slot).is_none() {
            return Ok(None);
        }
        self.resolve_slot_path(slot, None)
            .map(Some)
            .map_err(|error| UiLifecycleError::Registry(error.to_string()))
    }
    fn begin_root_replacement_open(
        &mut self,
        lifecycle: UiLifecycleState,
        target: UiOpenTarget,
    ) -> Result<u64, UiLifecycleError> {
        if let Some((id, _)) = self
            .pending
            .iter()
            .find(|(_, pending)| pending.replacement_lifecycle == Some(lifecycle))
        {
            return Ok(*id);
        }
        self.begin_open_internal(UiCommandSource::Host, target, true, Some(lifecycle), false)
    }

    pub fn begin_root_replacement(
        &mut self,
        lifecycle: UiLifecycleState,
    ) -> Result<UiRootReplacement, UiLifecycleError> {
        if let Some((id, _)) = self
            .pending
            .iter()
            .find(|(_, pending)| pending.replacement_lifecycle == Some(lifecycle))
        {
            return Ok(UiRootReplacement::Pending(UiInstanceId(*id)));
        }
        // Opens staged from the old Root can never survive this replacement.
        // Cancel them now so the platform adapter does not spend a WebView2
        // creation slot loading a document that will be stale at commit.
        self.pending
            .retain(|_, pending| pending.prefetched || pending.replacement_lifecycle.is_some());
        let Some(target) = self.configured_root_target_for(lifecycle)? else {
            return self
                .replace_root(lifecycle)
                .map(UiRootReplacement::Completed);
        };
        self.begin_root_replacement_open(lifecycle, target)
            .map(|id| UiRootReplacement::Pending(UiInstanceId(id)))
    }

    pub fn begin_configured_root(&mut self) -> Result<Option<u64>, UiLifecycleError> {
        let Some(target) = self.configured_root_target_for(self.lifecycle_state)? else {
            return Ok(None);
        };
        self.begin_open_internal(UiCommandSource::Host, target, true, None, false)
            .map(Some)
    }
    pub fn open_configured_root(&mut self) -> Result<Option<UiInstanceId>, UiLifecycleError> {
        let Some(pending) = self.begin_configured_root()? else {
            return Ok(None);
        };
        self.commit_open(pending).map(Some)
    }

    pub fn unload_definitions(
        &mut self,
        definitions: impl IntoIterator<Item = String>,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let definitions = definitions.into_iter().collect::<BTreeSet<_>>();
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
        let targets = self
            .instances
            .values()
            .filter(|instance| definitions.contains(&instance.definition))
            .map(|instance| instance.id.0)
            .collect::<Vec<_>>();
        let destroyed = if targets.is_empty() {
            Vec::new()
        } else {
            let registry = &self.registry;
            let plan = self
                .tree
                .plan_destruction(targets, false, |_, definition| {
                    definition
                        .as_ref()
                        .and_then(|name| registry.resource(name))
                        .is_some_and(|definition| definition.lifecycle_independent)
                })
                .map_err(|error| UiLifecycleError::InternalTree(format!("{error:?}")))?;
            let outcome = self
                .tree
                .commit(plan)
                .map_err(|error| UiLifecycleError::InternalTree(format!("{error:?}")))?;
            let destroyed = outcome
                .destroyed
                .into_iter()
                .filter(|node| *node != 0)
                .map(UiInstanceId)
                .collect::<Vec<_>>();
            self.remove_instances(&destroyed);
            destroyed
        };
        self.pending.retain(|_, pending| {
            let source_removed = match pending.source {
                UiCommandSource::Host => false,
                UiCommandSource::WebView(source) => self
                    .instances
                    .get(&source)
                    .is_none_or(|instance| definitions.contains(&instance.definition)),
            };
            !definitions.contains(&pending.target.resource) && !source_removed
        });
        self.registry
            .slots
            .retain(|_, definition| !definitions.contains(definition));
        self.registry
            .resources
            .retain(|definition, _| !definitions.contains(definition));
        for definition in &definitions {
            debug_assert_eq!(self.live_count(definition), 0);
            self.live_counts.remove(definition);
        }
        Ok(destroyed)
    }
    fn recompute_presentation(&mut self) {
        for instance in self.instances.values_mut() {
            instance.visible = true;
        }
        // At each ownership group, the most recently focused exclusive child
        // wins. Its parent and competing branches are hidden, not unloaded.
        let parents = std::iter::once(0)
            .chain(self.instances.keys().map(|id| id.0))
            .collect::<Vec<_>>();
        for parent in parents {
            let exclusive = self
                .tree
                .children(parent)
                .filter_map(|child| {
                    let id = UiInstanceId(child);
                    self.instances.get(&id).and_then(|instance| {
                        self.registry
                            .resource(&instance.definition)
                            .is_some_and(|definition| {
                                definition.presentation == PresentationMode::Exclusive
                            })
                            .then_some(id)
                    })
                })
                .collect::<Vec<_>>();
            let Some(winner) = exclusive
                .iter()
                .max_by_key(|id| {
                    self.focus_history
                        .iter()
                        .position(|candidate| candidate == *id)
                        .unwrap_or(0)
                })
                .copied()
            else {
                continue;
            };
            if parent != 0 {
                if let Some(instance) = self.instances.get_mut(&UiInstanceId(parent)) {
                    instance.visible = false;
                }
            }
            for sibling in self.tree.children(parent).collect::<Vec<_>>() {
                if sibling != winner.0 {
                    self.set_subtree_visibility(sibling, false);
                }
            }
        }
        self.focus_history
            .retain(|id| self.instances.contains_key(id));
    }

    fn set_subtree_visibility(&mut self, node: u64, visible: bool) {
        if let Some(instance) = self.instances.get_mut(&UiInstanceId(node)) {
            instance.visible = visible;
        }
        let children = self.tree.children(node).collect::<Vec<_>>();
        for child in children {
            self.set_subtree_visibility(child, visible);
        }
    }

    fn remove_instances(&mut self, ids: &[UiInstanceId]) {
        let destroyed = ids.iter().copied().collect::<BTreeSet<_>>();
        for id in ids {
            if let Some(instance) = self.instances.remove(id) {
                let count = self
                    .live_counts
                    .get_mut(&instance.definition)
                    .expect("committed count");
                *count -= 1;
                self.adapters.remove(id);
            }
        }
        self.pending.retain(|_, pending| match pending.source {
            UiCommandSource::Host => true,
            UiCommandSource::WebView(source) => !destroyed.contains(&source),
        });
        self.focus_history
            .retain(|id| self.instances.contains_key(id));
        self.recompute_presentation();
    }
}

#[cfg(target_os = "windows")]
fn webview2_resource_url(resource: &str, path: &str) -> String {
    // Match Wry's Windows custom-protocol workaround. `WebView::load_url`
    // does not apply this conversion (only builder-time URLs do).
    format!("http://roundo-ui.resource/{resource}/{path}")
}

fn top_level_navigation_allowed(source_resource: &str, url: &str) -> bool {
    let custom = format!("roundo-ui://resource/{source_resource}/");
    let webview2 = format!("http://roundo-ui.resource/{source_resource}/");
    url.starts_with(&custom) || url.starts_with(&webview2)
}

/// Composition plugin. It owns registry loading and the open-resource seam;
/// platform-specific Wry integration can consume `UiLifecycleManager` without
/// leaking registry details to the client composition root.
pub struct RoundoWebUiPlugin {
    mods_root: PathBuf,
    command_io: Option<JsonRequestResponseIo<Value>>,
}
impl RoundoWebUiPlugin {
    pub fn from_current_dir() -> Self {
        Self {
            mods_root: std::env::current_dir()
                .expect("current directory is unavailable")
                .join("mods"),
            command_io: None,
        }
    }
    pub fn from_current_dir_with_command_io(command_io: JsonRequestResponseIo<Value>) -> Self {
        Self {
            mods_root: std::env::current_dir()
                .expect("current directory is unavailable")
                .join("mods"),
            command_io: Some(command_io),
        }
    }
    pub fn with_mods_root(path: impl Into<PathBuf>) -> Self {
        Self {
            mods_root: path.into(),
            command_io: None,
        }
    }
}
impl Plugin for RoundoWebUiPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_os = "windows")]
        app.insert_resource(bevy_winit::WinitSettings {
            // On Windows, Bevy implements `Continuous` for a visible window by
            // waiting for parent-window redraw requests. A WebView2 child HWND
            // can satisfy its own paint cycle without producing that parent
            // redraw, leaving queued IPC commands and staged loads asleep until
            // the user touches the title bar. A bounded reactive deadline gives
            // the host a real timer wake while retaining event-driven updates.
            focused_mode: bevy_winit::UpdateMode::reactive(Duration::from_millis(16)),
            unfocused_mode: bevy_winit::UpdateMode::reactive(Duration::from_millis(16)),
        });
        let mods = LoadedMods::discover(&self.mods_root).unwrap_or_else(|error| {
            panic!(
                "cannot load Mods from {}: {error}",
                self.mods_root.display()
            )
        });
        let registry = UiRegistry::load(&mods)
            .unwrap_or_else(|error| panic!("cannot load Web UI registry: {error}"));
        let mut manager = UiLifecycleManager::new(registry);
        #[cfg(target_os = "windows")]
        manager.begin_configured_root().unwrap_or_else(|error| {
            panic!("cannot stage configured disconnected Root UI: {error}")
        });
        #[cfg(not(target_os = "windows"))]
        manager
            .open_configured_root()
            .unwrap_or_else(|error| panic!("cannot open configured disconnected Root UI: {error}"));
        app.insert_resource(manager);
        app.add_message::<RecoveryActionRequest>();
        app.insert_non_send(UiNavigationExecutor::default());
        #[cfg(target_os = "windows")]
        app.insert_non_send(WebUiCommandEndpoint {
            io: self.command_io.clone(),
        });
        #[cfg(target_os = "windows")]
        app.add_systems(
            bevy::app::Update,
            (
                retire_superseded_staged_webviews,
                stage_pending_webview,
                advance_staged_webviews,
                schedule_graph_prefetches,
                sync_recovery_surface,
                sync_committed_navigation,
                resize_webview,
                apply_windows_input_mode,
                resolve_webui_commands,
            )
                .chain(),
        );
    }
}

#[cfg(target_os = "windows")]
struct WebViewOverlay {
    webview: wry::WebView,
    pending: Arc<Mutex<Vec<PendingCommand>>>,
    load_state: Arc<Mutex<StagedLoadState>>,
    last_visible: bool,
    last_focused: bool,
    last_interaction_mode: Option<ClientInteractionMode>,
    last_bounds: Option<UiBounds>,
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct StagedLoadState {
    navigation_result: Option<Result<(), String>>,
    bridge_ready: bool,
    focus_requested: bool,
    finished_url: Option<String>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Eq, PartialEq)]
enum StagedReadiness {
    Pending,
    Commit,
    Failed(String),
    Timeout,
    Stale,
}

#[cfg(target_os = "windows")]
fn staged_readiness(
    pending_live: bool,
    navigation_result: Option<&Result<(), String>>,
    bridge_ready: bool,
    elapsed: Duration,
    timeout: Duration,
) -> StagedReadiness {
    if !pending_live {
        return StagedReadiness::Stale;
    }
    match navigation_result {
        Some(Err(error)) => StagedReadiness::Failed(error.clone()),
        Some(Ok(())) if bridge_ready => StagedReadiness::Commit,
        _ if elapsed >= timeout => StagedReadiness::Timeout,
        _ => StagedReadiness::Pending,
    }
}

#[cfg(target_os = "windows")]
struct RecoveryOverlay {
    webview: wry::WebView,
    actions: Arc<Mutex<Vec<RecoveryAction>>>,
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct StagedCommandGate {
    admitted: bool,
    queued: Vec<String>,
    overflow_logged: bool,
}

#[cfg(target_os = "windows")]
impl StagedCommandGate {
    fn queue_until_commit(&mut self, body: &str) -> bool {
        if self.admitted {
            false
        } else {
            if self.queued.len() < MAX_PREPARED_COMMANDS {
                self.queued.push(body.to_owned());
            } else if !self.overflow_logged {
                log::warn!(
                    "prepared Web UI command gate reached its {}-command limit",
                    MAX_PREPARED_COMMANDS
                );
                self.overflow_logged = true;
            }
            true
        }
    }

    fn commit(&mut self) -> Vec<String> {
        self.admitted = true;
        std::mem::take(&mut self.queued)
    }
}

#[cfg(target_os = "windows")]
struct StagedWebView {
    webview: wry::WebView,
    target_url: Option<String>,
    pending_commands: Arc<Mutex<Vec<PendingCommand>>>,
    command_gate: Arc<Mutex<StagedCommandGate>>,
    command_io: Option<JsonRequestResponseIo<Value>>,
    command_source: UiInstanceId,
    load_state: Arc<Mutex<StagedLoadState>>,
    created_at: Instant,
}

/// Synchronous navigation interface for the client-command seam. It is a
/// NonSend resource, keeping the Wry child WebView on the Bevy main thread.
struct DeferredOpenResponse {
    sender: ResponseSender<Value>,
    success: Value,
}

pub struct UiNavigationExecutor {
    last_open_pending: Option<UiInstanceId>,
    load_timeout: Duration,
    /// Each committed instance owns one physical child WebView. No navigation
    /// operation is allowed to reuse another instance's document.
    #[cfg(target_os = "windows")]
    committed: BTreeMap<UiInstanceId, WebViewOverlay>,
    /// Presentations from the replaced Root remain physically visible until
    /// the replacement Root has itself been made visible. They are no longer
    /// live command sources in the lifecycle model.
    #[cfg(target_os = "windows")]
    retiring: BTreeMap<UiInstanceId, WebViewOverlay>,
    #[cfg(target_os = "windows")]
    retained_root_presentations: Vec<UiInstanceId>,
    #[cfg(target_os = "windows")]
    staged: BTreeMap<u64, StagedWebView>,
    /// Fully loaded graph-prefetched documents. They remain command-gated and
    /// consume neither a lifecycle instance nor `max_instances` until claimed.
    #[cfg(target_os = "windows")]
    prepared: BTreeMap<u64, StagedWebView>,
    /// Superseded staged WebViews are moved here instead of dropped while a
    /// WebView2 navigation callback may still be on the Win32 stack. The next
    /// host update retires them, followed by one creation-free update so COM
    /// teardown and the next controller creation cannot share a callback turn.
    #[cfg(target_os = "windows")]
    superseded_staged: BTreeMap<u64, StagedWebView>,
    #[cfg(target_os = "windows")]
    webview_creation_cooldown: bool,
    #[cfg(target_os = "windows")]
    deferred_open_responses: BTreeMap<u64, DeferredOpenResponse>,
    #[cfg(target_os = "windows")]
    recovery: Option<RecoveryOverlay>,
}
impl Default for UiNavigationExecutor {
    fn default() -> Self {
        Self {
            last_open_pending: None,
            load_timeout: Duration::from_secs(15),
            #[cfg(target_os = "windows")]
            committed: BTreeMap::new(),
            #[cfg(target_os = "windows")]
            retiring: BTreeMap::new(),
            #[cfg(target_os = "windows")]
            retained_root_presentations: Vec::new(),
            #[cfg(target_os = "windows")]
            staged: BTreeMap::new(),
            #[cfg(target_os = "windows")]
            prepared: BTreeMap::new(),
            #[cfg(target_os = "windows")]
            superseded_staged: BTreeMap::new(),
            #[cfg(target_os = "windows")]
            webview_creation_cooldown: false,
            #[cfg(target_os = "windows")]
            deferred_open_responses: BTreeMap::new(),
            #[cfg(target_os = "windows")]
            recovery: None,
        }
    }
}
impl UiNavigationExecutor {
    #[cfg(target_os = "windows")]
    fn supersede_staged_webview(&mut self, id: u64) {
        if let Some(staged) = self
            .staged
            .remove(&id)
            .or_else(|| self.prepared.remove(&id))
        {
            self.superseded_staged.insert(id, staged);
        }
    }

    #[cfg(target_os = "windows")]
    fn begin_root_presentation_handoff(
        &mut self,
        presentations: impl IntoIterator<Item = UiInstanceId>,
    ) {
        for id in presentations {
            if !self.retained_root_presentations.contains(&id) {
                self.retained_root_presentations.push(id);
            }
        }
    }

    /// Returns true only when a visible replacement releases the retained
    /// presentation. A pending or hidden replacement must leave coverage in
    /// place.
    #[cfg(target_os = "windows")]
    fn finish_root_presentation_handoff(&mut self, replacement_visible: bool) -> bool {
        if !replacement_visible || self.retained_root_presentations.is_empty() {
            return false;
        }
        for (id, overlay) in &self.retiring {
            log::info!(
                "Destroying retained Web UI instance {} after Root presentation handoff",
                id.get()
            );
            dispatch_lifecycle_event(&overlay.webview, "roundo:destroying", "destroying");
        }
        self.retiring.clear();
        self.retained_root_presentations.clear();
        true
    }

    pub fn set_load_timeout(&mut self, timeout: Duration) {
        assert!(!timeout.is_zero(), "UI load timeout must be positive");
        self.load_timeout = timeout;
    }

    pub fn open(
        &mut self,
        state: &mut UiLifecycleManager,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        // Production opens remain Pending until the staged WebView reports
        // both main-document completion and the injected bridge handshake.
        // The pending id is process-unique and becomes the committed instance
        // id only after `commit_open` succeeds.
        #[cfg(target_os = "windows")]
        let opened = match state.claim_prefetch(source, &target)? {
            Some(pending) => {
                log::debug!(
                    "claimed prepared Web UI candidate {pending} for `{}`",
                    target.resource()
                );
                Ok(UiInstanceId::from_host_id(pending))
            }
            None => state
                .begin_open(source, target)
                .map(UiInstanceId::from_host_id),
        };
        #[cfg(not(target_os = "windows"))]
        let opened = state.open(source, target);
        if let Ok(id) = &opened {
            self.last_open_pending = Some(*id);
        }
        opened
    }

    pub fn prepare_open_response(&mut self) {
        self.last_open_pending = None;
    }

    pub fn take_last_open(&mut self) -> Option<UiInstanceId> {
        self.last_open_pending.take()
    }

    pub fn defer_open_response(
        &mut self,
        pending: UiInstanceId,
        sender: ResponseSender<Value>,
        success: Value,
    ) {
        #[cfg(target_os = "windows")]
        {
            self.deferred_open_responses
                .insert(pending.get(), DeferredOpenResponse { sender, success });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = pending;
            let _ = sender.respond(success);
        }
    }

    #[cfg(target_os = "windows")]
    fn resolve_open_response(&mut self, pending: u64, error: Option<(&str, String)>) {
        let Some(response) = self.deferred_open_responses.remove(&pending) else {
            return;
        };
        let value = match error {
            None => response.success,
            Some((code, message)) => json!({
                "version": 1,
                "command": "ui.open",
                "ok": false,
                "error": {"code": code, "message": message},
            }),
        };
        if response.sender.respond(value).is_err() {
            log::debug!("ui.open caller disappeared before staged load completed");
        }
    }

    pub fn open_configured_root(
        &mut self,
        state: &mut UiLifecycleManager,
    ) -> Result<Option<UiInstanceId>, UiLifecycleError> {
        #[cfg(target_os = "windows")]
        {
            let pending = state
                .begin_configured_root()
                .map(|pending| pending.map(UiInstanceId::from_host_id));
            if matches!(pending, Ok(None)) {
                // A Root UI is optional. The game/world itself is the new
                // presentation, so no WebView readiness edge will release the
                // retained old Root for us.
                self.finish_root_presentation_handoff(true);
            }
            pending
        }
        #[cfg(not(target_os = "windows"))]
        {
            state.open_configured_root()
        }
    }

    pub fn back(
        &mut self,
        state: &mut UiLifecycleManager,
        source: UiInstanceId,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let destroyed = state.back(source)?;
        #[cfg(target_os = "windows")]
        {
            let restore_parent_focus = state.focused_declaration().is_none();
            let cancelled = self
                .deferred_open_responses
                .keys()
                .copied()
                .filter(|id| state.pending_descriptor(*id).is_none())
                .collect::<Vec<_>>();
            for id in cancelled {
                self.resolve_open_response(
                    id,
                    Some(("stale_ui_instance", "source UI was destroyed".into())),
                );
                self.supersede_staged_webview(id);
            }
            for id in &destroyed {
                if let Some(overlay) = self.committed.remove(id) {
                    if restore_parent_focus && *id == source {
                        match overlay.webview.focus_parent() {
                            Ok(()) => log::debug!(
                                "restored game-window focus while closing UI instance {}",
                                id.get()
                            ),
                            Err(error) => log::error!(
                                "cannot restore game-window focus while closing Web UI: {error}"
                            ),
                        }
                    }
                    dispatch_lifecycle_event(&overlay.webview, "roundo:destroying", "destroying");
                    drop(overlay);
                }
            }
        }
        Ok(destroyed)
    }

    pub fn unload_definitions(
        &mut self,
        state: &mut UiLifecycleManager,
        definitions: impl IntoIterator<Item = String>,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let destroyed = state.unload_definitions(definitions)?;
        #[cfg(target_os = "windows")]
        {
            let cancelled = self
                .deferred_open_responses
                .keys()
                .copied()
                .filter(|id| state.pending_descriptor(*id).is_none())
                .collect::<Vec<_>>();
            for id in cancelled {
                self.resolve_open_response(
                    id,
                    Some(("stale_ui_instance", "UI definition was unloaded".into())),
                );
            }
            let stale_staged = self
                .staged
                .keys()
                .chain(self.prepared.keys())
                .copied()
                .filter(|id| state.pending_descriptor(*id).is_none())
                .collect::<Vec<_>>();
            for id in stale_staged {
                self.supersede_staged_webview(id);
            }
            for id in &destroyed {
                if let Some(overlay) = self.committed.remove(id) {
                    dispatch_lifecycle_event(&overlay.webview, "roundo:destroying", "destroying");
                    drop(overlay);
                }
            }
        }
        Ok(destroyed)
    }

    pub fn cancel_root_replacement(&mut self, state: &mut UiLifecycleManager) {
        #[cfg(target_os = "windows")]
        for id in state.cancel_root_replacement() {
            self.supersede_staged_webview(id);
            self.resolve_open_response(
                id,
                Some((
                    "stale_ui_instance",
                    "authoritative Root target changed".into(),
                )),
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = state.cancel_root_replacement();
    }

    pub fn replace_root(
        &mut self,
        state: &mut UiLifecycleManager,
        lifecycle: UiLifecycleState,
    ) -> Result<UiRootReplacement, UiLifecycleError> {
        let replacement = state.begin_root_replacement(lifecycle)?;
        #[cfg(target_os = "windows")]
        {
            let stale_staged = self
                .staged
                .keys()
                .chain(self.prepared.keys())
                .copied()
                .filter(|id| state.pending_descriptor(*id).is_none())
                .collect::<Vec<_>>();
            for id in stale_staged {
                // Do not drop a WebView2 controller from inside the same host
                // update in which its initial navigation callback may be on
                // the Win32 stack. Moving ownership is non-destructive; the
                // the next host update retires it before another controller is created.
                self.supersede_staged_webview(id);
            }
            // Keep deferred responses unresolved during the handoff. Resolving
            // one evaluates JavaScript in the old WebView and can re-enter
            // WebView2 while another controller's NavigationStarting callback
            // is active. The transactional commit resolves every now-stale
            // response after the replacement document has finished loading.
        }
        #[cfg(target_os = "windows")]
        if let UiRootReplacement::Completed(destroyed) = &replacement {
            let pending = self
                .deferred_open_responses
                .keys()
                .copied()
                .collect::<Vec<_>>();
            for id in pending {
                self.resolve_open_response(
                    id,
                    Some(("stale_ui_instance", "UI root was replaced".into())),
                );
            }
            let stale_staged = self
                .staged
                .keys()
                .chain(self.prepared.keys())
                .copied()
                .collect::<Vec<_>>();
            for id in stale_staged {
                self.supersede_staged_webview(id);
            }
            self.recovery = None;
            for id in destroyed {
                if let Some(overlay) = self.committed.remove(id) {
                    dispatch_lifecycle_event(&overlay.webview, "roundo:destroying", "destroying");
                }
            }
        }
        Ok(replacement)
    }
}

#[cfg(target_os = "windows")]
struct WebUiCommandEndpoint {
    io: Option<JsonRequestResponseIo<Value>>,
}

struct PendingCommand {
    request_id: u64,
    command_name: String,
    submitted_at: Instant,
    timeout_logged: bool,
    call: Option<RequestCall<Value>>,
    immediate: Option<Value>,
}

#[cfg(target_os = "windows")]
fn claim_webview_creation_turn(
    active_transactions: usize,
    retiring_transactions: usize,
    cooldown: &mut bool,
) -> bool {
    if active_transactions != 0 || retiring_transactions != 0 {
        return false;
    }
    if std::mem::take(cooldown) {
        return false;
    }
    true
}

#[cfg(target_os = "windows")]
fn retire_superseded_staged_webviews(world: &mut bevy::prelude::World) {
    let retired = {
        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
        if executor.superseded_staged.is_empty() {
            return;
        }
        let retired = executor
            .superseded_staged
            .keys()
            .copied()
            .collect::<Vec<_>>();
        executor.superseded_staged.clear();
        executor.webview_creation_cooldown = true;
        retired
    };
    let mut executor = world.non_send_mut::<UiNavigationExecutor>();
    for id in retired {
        executor.resolve_open_response(
            id,
            Some(("stale_ui_instance", "UI root was replaced".into())),
        );
    }
}

#[cfg(target_os = "windows")]
fn schedule_graph_prefetches(mut state: bevy::prelude::ResMut<UiLifecycleManager>) {
    let targets = state
        .instances()
        .filter(|instance| instance.visible && instance.loaded)
        .filter_map(|instance| state.registry.resource(&instance.definition))
        .flat_map(|definition| definition.prefetch.iter().cloned())
        .collect::<BTreeSet<_>>();
    let descriptors = state.pending_descriptors();
    let mut available = MAX_PREPARED_UI_CANDIDATES.saturating_sub(
        descriptors
            .iter()
            .filter(|pending| pending.prefetched)
            .count(),
    );
    let pending_targets = descriptors
        .into_iter()
        .map(|pending| pending.target)
        .collect::<BTreeSet<_>>();
    for definition in targets {
        if available == 0 {
            break;
        }
        let Some(resource) = state.registry.resource(&definition) else {
            continue;
        };
        if state.live_count(&definition) >= resource.max_instances {
            continue;
        }
        let target = match state.resolve_resource_path(&definition, None) {
            Ok(target) => target,
            Err(error) => {
                log::warn!("cannot resolve graph-prefetch target `{definition}`: {error}");
                continue;
            }
        };
        if pending_targets.contains(&target) {
            continue;
        }
        match state.begin_prefetch(target) {
            Ok(id) => {
                available -= 1;
                log::debug!("queued graph-prefetch candidate {id} for `{definition}`");
            }
            Err(UiLifecycleError::UiInstanceLimit | UiLifecycleError::DuplicatePendingOpen) => {}
            Err(error) => log::warn!("cannot queue graph-prefetch for `{definition}`: {error}"),
        }
    }
}

#[cfg(target_os = "windows")]
fn stage_pending_webview(world: &mut bevy::prelude::World) {
    let entity = world.query_filtered::<bevy::prelude::Entity, bevy::ecs::query::With<bevy::window::PrimaryWindow>>().iter(world).next();
    let Some(entity) = entity else {
        return;
    };
    let may_create = {
        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
        let active = executor.staged.len();
        let retiring = executor.superseded_staged.len();
        let cooldown = &mut executor.webview_creation_cooldown;
        claim_webview_creation_turn(active, retiring, cooldown)
    };
    if !may_create {
        return;
    }
    let descriptor = {
        let state = world.resource::<UiLifecycleManager>();
        let executor = world.non_send::<UiNavigationExecutor>();
        state.pending_descriptors().into_iter().find(|pending| {
            !executor.staged.contains_key(&pending.id)
                && !executor.prepared.contains_key(&pending.id)
                && !executor
                    .committed
                    .contains_key(&UiInstanceId::from_host_id(pending.id))
        })
    };
    let Some(descriptor) = descriptor else {
        return;
    };
    let (registry, source_definition, transparent) = {
        let state = world.resource::<UiLifecycleManager>();
        let definition = state
            .registry
            .resource(descriptor.target.resource())
            .expect("pending definition remains registered");
        (
            Arc::new(state.registry.clone()),
            definition.name.clone(),
            definition.world_visibility == UiWorldVisibility::Visible,
        )
    };
    let url = webview2_resource_url(descriptor.target.resource(), &descriptor.target.path);
    let protocol_registry = Arc::clone(&registry);
    let navigation_definition = source_definition.clone();
    let protocol_definition = source_definition.clone();
    let allow_initial_blank = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let command_io = world
        .get_non_send::<WebUiCommandEndpoint>()
        .and_then(|endpoint| endpoint.io.clone());
    let staged_command_io = command_io.clone();
    let event_loop_proxy =
        std::ops::Deref::deref(world.resource::<bevy_winit::EventLoopProxyWrapper>()).clone();
    let ipc_event_loop_proxy = event_loop_proxy.clone();
    let page_load_event_loop_proxy = event_loop_proxy.clone();
    let native_event_loop_proxy = event_loop_proxy;
    let pending_commands = Arc::new(Mutex::new(Vec::new()));
    let ipc_pending_commands = Arc::clone(&pending_commands);
    let command_gate = Arc::new(Mutex::new(StagedCommandGate::default()));
    let ipc_command_gate = Arc::clone(&command_gate);
    let parent_hwnd = Arc::new(std::sync::atomic::AtomicIsize::new(0));
    let ipc_parent_hwnd = Arc::clone(&parent_hwnd);
    let load_state = Arc::new(Mutex::new(StagedLoadState::default()));
    let page_load_state = Arc::clone(&load_state);
    let ipc_load_state = Arc::clone(&load_state);
    let native_load_state = Arc::clone(&load_state);
    let starting_load_state = Arc::clone(&load_state);
    let native_navigation_definition = source_definition.clone();
    let frame_navigation_definition = source_definition.clone();
    let native_target_path = descriptor.target.path.clone();
    let command_source = UiInstanceId::from_host_id(descriptor.id);
    let initialization_script = webui_initialization_script(&source_definition);
    // WebView2 controller creation can move Win32 keyboard focus even when the
    // child starts hidden. Speculative work must be presentation-neutral: a
    // prepared candidate may never steal WASD/Escape from the active view.
    let previous_focus = if descriptor.prefetched {
        Some(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() })
    } else {
        None
    };
    let result = bevy_winit::WINIT_WINDOWS.with(|all_windows| {
        let all_windows = all_windows.borrow();
        let window = all_windows
            .get_window(entity)
            .ok_or_else(|| "primary native window is not ready".to_string())?;
        let size = window.inner_size();
        let scale_factor = window.scale_factor();
        wry::WebViewBuilder::new()
            .with_bounds(webview_rect(UiBounds {
                x: 0,
                y: 0,
                width: (size.width as f64 / scale_factor).round() as u32,
                height: (size.height as f64 / scale_factor).round() as u32,
            }))
            .with_visible(false)
            .with_transparent(transparent)
            .with_url("about:blank")
            .with_navigation_handler(move |url| {
                let allowed = (url == "about:blank"
                    && allow_initial_blank.swap(false, std::sync::atomic::Ordering::AcqRel))
                    || top_level_navigation_allowed(&navigation_definition, &url);
                if !allowed {
                    log::warn!("blocked cross-resource or external top-level navigation: `{url}`");
                }
                allowed
            })
            .with_on_page_load_handler(move |event, url| {
                let phase = match event {
                    wry::PageLoadEvent::Started => "started",
                    wry::PageLoadEvent::Finished => {
                        page_load_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .finished_url = Some(url.clone());
                        // Initial about:blank completion is the gate for loading
                        // the real resource URL. Wake the Bevy loop immediately;
                        // otherwise staging may wait for the next pointer/window
                        // event and make navigation appear to require a click.
                        let _ = page_load_event_loop_proxy
                            .send_event(bevy_winit::WinitUserEvent::WakeUp);
                        "finished"
                    }
                };
                log::debug!("WebView2 page load {phase}: `{url}`");
            })
            .with_custom_protocol("roundo-ui".into(), move |_, request| {
                // The source Definition is bound by the host-owned WebView,
                // never inferred from a suppressible/spoofable Referer.
                protocol_response(
                    &protocol_registry,
                    Some(&protocol_definition),
                    request.uri(),
                )
            })
            .with_initialization_script(initialization_script)
            .with_ipc_handler(move |message| {
                if bridge_handshake(message.body()) {
                    ipc_load_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .bridge_ready = true;
                    invalidate_parent_window(&ipc_parent_hwnd);
                    let _ = ipc_event_loop_proxy.send_event(bevy_winit::WinitUserEvent::WakeUp);
                    return;
                }
                if bridge_focus_request(message.body()) {
                    ipc_load_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .focus_requested = true;
                    invalidate_parent_window(&ipc_parent_hwnd);
                    let _ = ipc_event_loop_proxy.send_event(bevy_winit::WinitUserEvent::WakeUp);
                    return;
                }
                let queue_until_commit = {
                    let mut gate = ipc_command_gate
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    gate.queue_until_commit(message.body())
                };
                if !queue_until_commit {
                    enqueue_webui_command(
                        &command_io,
                        &ipc_pending_commands,
                        Some(command_source),
                        message.body(),
                    );
                }
                // Waking the continuous event loop is sufficient for command polling.
                // Invalidating the entire parent window for every polling request makes
                // child WebView composition visibly flash on Windows.
                let _ = ipc_event_loop_proxy.send_event(bevy_winit::WinitUserEvent::WakeUp);
            })
            .build_as_child(&**window)
            .map_err(|error| error.to_string())
    });
    if let Some(previous_focus) = previous_focus.filter(|window| !window.is_invalid()) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
        if unsafe { GetFocus() } != previous_focus {
            unsafe {
                let _ = SetFocus(Some(previous_focus));
            }
            log::debug!("restored Win32 focus after preparing hidden Web UI candidate");
        }
    }
    match result {
        Ok(webview) => {
            use webview2_com::{
                Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_ERROR_STATUS,
                NavigationCompletedEventHandler, NavigationStartingEventHandler,
            };
            use wry::WebViewExtWindows;

            use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};
            let host_window = unsafe { GetAncestor(webview.hwnd(), GA_ROOT) };
            parent_hwnd.store(host_window.0 as isize, std::sync::atomic::Ordering::Release);

            let target_navigation_id = Arc::new(Mutex::new(None::<u64>));
            let starting_target_navigation_id = Arc::clone(&target_navigation_id);
            let completed_target_navigation_id = Arc::clone(&target_navigation_id);
            let core = webview.webview();
            let starting_handler =
                NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut raw_uri = windows::core::PWSTR::null();
                    unsafe { args.Uri(&mut raw_uri)? };
                    let uri = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
                    let mut navigation_id = 0;
                    unsafe { args.NavigationId(&mut navigation_id)? };
                    log::debug!(
                        "WebView2 native navigation starting: id={navigation_id}, uri=`{uri}`"
                    );
                    if same_definition_path(&native_navigation_definition, &uri).as_deref()
                        == Some(native_target_path.as_str())
                    {
                        *starting_target_navigation_id
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(navigation_id);
                        let mut load = starting_load_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        load.navigation_result = None;
                        load.bridge_ready = false;
                        load.finished_url = None;
                    }
                    Ok(())
                }));
            let completed_handler = NavigationCompletedEventHandler::create(Box::new(
                move |_sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut navigation_id = 0;
                    unsafe { args.NavigationId(&mut navigation_id)? };
                    let target_id = *completed_target_navigation_id
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    log::debug!(
                        "WebView2 native navigation completed: id={navigation_id}, target={target_id:?}"
                    );
                    if target_id != Some(navigation_id) {
                        return Ok(());
                    }
                    let mut success = windows::core::BOOL::default();
                    unsafe { args.IsSuccess(&mut success)? };
                    let result = if success.as_bool() {
                        Ok(())
                    } else {
                        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                        unsafe { args.WebErrorStatus(&mut status)? };
                        Err(format!("WebView2 navigation failed with status {status:?}"))
                    };
                    native_load_state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .navigation_result = Some(result);
                    let _ = native_event_loop_proxy.send_event(bevy_winit::WinitUserEvent::WakeUp);
                    Ok(())
                },
            ));
            let frame_handler =
                NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut raw_uri = windows::core::PWSTR::null();
                    unsafe { args.Uri(&mut raw_uri)? };
                    let uri = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
                    let allowed = frame_navigation_allowed(&frame_navigation_definition, &uri);
                    if !allowed {
                        log::warn!(
                            "blocked cross-Definition or external frame navigation: `{uri}`"
                        );
                        unsafe { args.SetCancel(true)? };
                    }
                    Ok(())
                }));
            let mut starting_token = 0;
            let mut completed_token = 0;
            let mut frame_token = 0;
            let handler_result = unsafe {
                core.add_NavigationStarting(&starting_handler, &mut starting_token)
                    .and_then(|_| {
                        core.add_NavigationCompleted(&completed_handler, &mut completed_token)
                    })
                    .and_then(|_| {
                        core.add_FrameNavigationStarting(&frame_handler, &mut frame_token)
                    })
            };
            let load_result = handler_result.map_err(|error| {
                format!("cannot install WebView2 navigation status handlers: {error}")
            });
            if let Err(error) = load_result {
                let _ = world
                    .resource_mut::<UiLifecycleManager>()
                    .fail_open(descriptor.id, error.clone());
                world
                    .non_send_mut::<UiNavigationExecutor>()
                    .resolve_open_response(
                        descriptor.id,
                        Some(("ui_navigation_failed", error.clone())),
                    );
                log::error!("{error}");
                return;
            }
            world.non_send_mut::<UiNavigationExecutor>().staged.insert(
                descriptor.id,
                StagedWebView {
                    webview,
                    target_url: Some(url),
                    pending_commands,
                    command_gate,
                    command_io: staged_command_io,
                    command_source,
                    load_state,
                    created_at: Instant::now(),
                },
            );
        }
        Err(error) => {
            let _ = world
                .resource_mut::<UiLifecycleManager>()
                .fail_open(descriptor.id, error.clone());
            world
                .non_send_mut::<UiNavigationExecutor>()
                .resolve_open_response(
                    descriptor.id,
                    Some(("ui_navigation_failed", error.clone())),
                );
            log::error!(
                "cannot create WebView2 child: {error}. Install Microsoft Edge WebView2 Runtime. The client will exit rather than pretend a Mod UI loaded."
            );
            let _ = world.write_message(bevy::app::AppExit::error());
        }
    }
}

#[cfg(target_os = "windows")]
fn advance_staged_webviews(world: &mut bevy::prelude::World) {
    // A real open claims a prepared document by clearing its speculative flag.
    // Move it back through the ordinary commit path so lifecycle admission,
    // command-gate release and presentation stay identical to cold opens.
    let (claimed, stale) = {
        let state = world.resource::<UiLifecycleManager>();
        let executor = world.non_send::<UiNavigationExecutor>();
        let mut claimed = Vec::new();
        let mut stale = Vec::new();
        for id in executor.prepared.keys().copied() {
            match state.pending_descriptor(id) {
                Some(descriptor) if !descriptor.prefetched => claimed.push(id),
                Some(_) => {}
                None => stale.push(id),
            }
        }
        (claimed, stale)
    };
    {
        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
        for id in claimed {
            if let Some(prepared) = executor.prepared.remove(&id) {
                resume_prepared_webview(&prepared.webview);
                executor.staged.insert(id, prepared);
            }
        }
        for id in stale {
            executor.supersede_staged_webview(id);
        }
    }

    // Wry starts an asynchronous initial about:blank navigation while building
    // a WebView. Starting the real URL before that finishes lets the initial
    // navigation cancel it. Wait for about:blank, then navigate with native
    // status handlers already installed.
    {
        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
        for (id, staged) in &mut executor.staged {
            let initial_finished = staged
                .load_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finished_url
                .as_deref()
                == Some("about:blank");
            if !initial_finished {
                continue;
            }
            let Some(url) = staged.target_url.take() else {
                continue;
            };
            if let Err(error) = staged.webview.load_url(&url) {
                staged
                    .load_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .navigation_result = Some(Err(format!(
                    "cannot start WebView2 navigation for pending UI {id}: {error}"
                )));
            }
        }
    }

    enum Resolution {
        Commit(u64),
        Prepared(u64),
        Failed(u64, String),
        Timeout(u64),
        Stale(u64),
    }
    let resolutions = {
        let state = world.resource::<UiLifecycleManager>();
        let executor = world.non_send::<UiNavigationExecutor>();
        executor
            .staged
            .iter()
            .filter_map(|(id, staged)| {
                let load = staged
                    .load_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let descriptor = state.pending_descriptor(*id);
                match staged_readiness(
                    descriptor.is_some(),
                    load.navigation_result.as_ref(),
                    load.bridge_ready,
                    staged.created_at.elapsed(),
                    executor.load_timeout,
                ) {
                    StagedReadiness::Pending => None,
                    StagedReadiness::Commit if descriptor.is_some_and(|value| value.prefetched) => {
                        Some(Resolution::Prepared(*id))
                    }
                    StagedReadiness::Commit => Some(Resolution::Commit(*id)),
                    StagedReadiness::Failed(error) => Some(Resolution::Failed(*id, error)),
                    StagedReadiness::Timeout => Some(Resolution::Timeout(*id)),
                    StagedReadiness::Stale => Some(Resolution::Stale(*id)),
                }
            })
            .collect::<Vec<_>>()
    };
    for resolution in resolutions {
        match resolution {
            Resolution::Prepared(pending_id) => {
                let prepared = world
                    .non_send_mut::<UiNavigationExecutor>()
                    .staged
                    .remove(&pending_id);
                if let Some(prepared) = prepared {
                    suspend_prepared_webview(&prepared.webview);
                    world
                        .non_send_mut::<UiNavigationExecutor>()
                        .prepared
                        .insert(pending_id, prepared);
                    log::debug!("prepared Web UI candidate {pending_id} for graph-prefetch");
                }
            }
            Resolution::Commit(pending_id) => {
                let replacing_root = world
                    .resource::<UiLifecycleManager>()
                    .pending_root_replacement(pending_id)
                    .is_some();
                let commit = if replacing_root {
                    world
                        .resource_mut::<UiLifecycleManager>()
                        .commit_root_replacement(pending_id)
                        .map(|commit| (commit.instance, commit.destroyed))
                } else {
                    world
                        .resource_mut::<UiLifecycleManager>()
                        .commit_open(pending_id)
                        .map(|instance| (instance, Vec::new()))
                };
                let staged = world
                    .non_send_mut::<UiNavigationExecutor>()
                    .staged
                    .remove(&pending_id);
                match (commit, staged) {
                    (Ok((instance, destroyed)), Some(staged)) => {
                        let queued = {
                            let mut gate = staged
                                .command_gate
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            gate.commit()
                        };
                        for body in queued {
                            enqueue_webui_command(
                                &staged.command_io,
                                &staged.pending_commands,
                                Some(staged.command_source),
                                &body,
                            );
                        }
                        let still_pending = world
                            .resource::<UiLifecycleManager>()
                            .pending_descriptors()
                            .into_iter()
                            .map(|pending| pending.id)
                            .collect::<BTreeSet<_>>();
                        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
                        if !destroyed.is_empty() {
                            let stale_staged = executor
                                .staged
                                .keys()
                                .chain(executor.prepared.keys())
                                .copied()
                                .filter(|id| !still_pending.contains(id))
                                .collect::<Vec<_>>();
                            for id in stale_staged {
                                executor.supersede_staged_webview(id);
                                executor.resolve_open_response(
                                    id,
                                    Some(("stale_ui_instance", "UI root was replaced".into())),
                                );
                            }
                            let stale_responses = executor
                                .deferred_open_responses
                                .keys()
                                .copied()
                                .filter(|id| *id != pending_id)
                                .collect::<Vec<_>>();
                            for id in stale_responses {
                                executor.resolve_open_response(
                                    id,
                                    Some(("stale_ui_instance", "UI root was replaced".into())),
                                );
                            }
                            let mut retained = Vec::new();
                            for id in &destroyed {
                                if let Some(overlay) = executor.committed.remove(id) {
                                    executor.retiring.insert(*id, overlay);
                                    retained.push(*id);
                                }
                            }
                            executor.begin_root_presentation_handoff(retained);
                            log::info!(
                                "Committed transactional Root replacement; destroyed_instances={}",
                                destroyed.len()
                            );
                        }
                        executor.committed.insert(
                            instance,
                            WebViewOverlay {
                                webview: staged.webview,
                                pending: staged.pending_commands,
                                load_state: staged.load_state,
                                last_visible: false,
                                last_focused: false,
                                last_interaction_mode: None,
                                last_bounds: None,
                            },
                        );
                        executor.resolve_open_response(pending_id, None);
                    }
                    (Err(error), staged) => {
                        drop(staged);
                        let code = match &error {
                            UiLifecycleError::UiInstanceLimit => "ui_instance_limit",
                            UiLifecycleError::LoadTimeout => "ui_load_timeout",
                            UiLifecycleError::Registry(_) => "invalid_ui_resource",
                            _ => "stale_ui_instance",
                        };
                        world
                            .non_send_mut::<UiNavigationExecutor>()
                            .resolve_open_response(pending_id, Some((code, error.to_string())));
                        log::warn!("discarded stale staged WebView {pending_id}: {error}");
                    }
                    (Ok(_), None) => unreachable!("staged WebView disappeared before commit"),
                }
            }
            Resolution::Failed(pending_id, error) => {
                world
                    .non_send_mut::<UiNavigationExecutor>()
                    .staged
                    .remove(&pending_id);
                let _ = world
                    .resource_mut::<UiLifecycleManager>()
                    .fail_open(pending_id, error.clone());
                world
                    .non_send_mut::<UiNavigationExecutor>()
                    .resolve_open_response(
                        pending_id,
                        Some(("ui_navigation_failed", error.clone())),
                    );
                log::error!("staged WebView {pending_id} failed navigation: {error}");
            }
            Resolution::Timeout(pending_id) => {
                world
                    .non_send_mut::<UiNavigationExecutor>()
                    .staged
                    .remove(&pending_id);
                let _ = world
                    .resource_mut::<UiLifecycleManager>()
                    .fail_open(pending_id, "UI load timed out before bridge handshake");
                world
                    .non_send_mut::<UiNavigationExecutor>()
                    .resolve_open_response(
                        pending_id,
                        Some((
                            "ui_load_timeout",
                            "UI load timed out before bridge handshake".into(),
                        )),
                    );
                log::error!("staged WebView {pending_id} exceeded the UI load timeout");
            }
            Resolution::Stale(pending_id) => {
                let mut executor = world.non_send_mut::<UiNavigationExecutor>();
                executor.supersede_staged_webview(pending_id);
                executor.resolve_open_response(
                    pending_id,
                    Some(("stale_ui_instance", "pending UI open became stale".into())),
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn suspend_prepared_webview(webview: &wry::WebView) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::ICoreWebView2_3, TrySuspendCompletedHandler,
    };
    use windows::core::Interface;
    use wry::WebViewExtWindows;

    let Ok(core): Result<ICoreWebView2_3, _> = webview.webview().cast() else {
        log::debug!("WebView2 suspension is unavailable for a prepared UI candidate");
        return;
    };
    let completed = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
    if let Err(error) = unsafe { core.TrySuspend(&completed) } {
        log::debug!("cannot suspend prepared Web UI candidate: {error}");
    }
}

#[cfg(target_os = "windows")]
fn resume_prepared_webview(webview: &wry::WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use windows::core::Interface;
    use wry::WebViewExtWindows;

    let Ok(core): Result<ICoreWebView2_3, _> = webview.webview().cast() else {
        return;
    };
    if let Err(error) = unsafe { core.Resume() } {
        log::debug!("cannot resume prepared Web UI candidate: {error}");
    }
}

#[cfg(target_os = "windows")]
const RECOVERY_HTML: &str = r#"<!doctype html><meta charset='utf-8'><title>Roundo UI recovery</title><style>html,body{margin:0;width:100%;height:100%;background:#111;color:#eee;font:16px sans-serif}main{height:100%;display:grid;place-content:center;text-align:center;gap:16px}button{padding:10px 20px;margin:4px}</style><main><h1>UI failed to load</h1><p>The game state is unchanged. Choose a controlled recovery action.</p><div><button onclick=send('retry')>Retry</button><button onclick=send('disconnect')>Disconnect</button><button onclick=send('quit')>Quit</button></div></main><script>function send(recovery_action){window.ipc.postMessage(JSON.stringify({recovery_action}))}</script>"#;

#[cfg(target_os = "windows")]
fn sync_recovery_surface(world: &mut bevy::prelude::World) {
    let actions = world
        .get_non_send_mut::<UiNavigationExecutor>()
        .and_then(|mut executor| {
            executor.recovery.as_mut().map(|surface| {
                std::mem::take(
                    &mut *surface
                        .actions
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                )
            })
        })
        .unwrap_or_default();
    for action in actions {
        match action {
            RecoveryAction::Retry => {
                match world.resource_mut::<UiLifecycleManager>().retry_recovery() {
                    Ok(_) => world.non_send_mut::<UiNavigationExecutor>().recovery = None,
                    Err(error) => log::error!("cannot retry Root UI recovery: {error}"),
                }
            }
            RecoveryAction::Disconnect | RecoveryAction::Quit => {
                let _ = world.write_message(RecoveryActionRequest(action));
            }
        }
    }

    if world
        .resource::<UiLifecycleManager>()
        .recovery_surface()
        .is_none()
    {
        world.non_send_mut::<UiNavigationExecutor>().recovery = None;
        return;
    }
    let entity = world.query_filtered::<bevy::prelude::Entity, bevy::ecs::query::With<bevy::window::PrimaryWindow>>().iter(world).next();
    let Some(entity) = entity else {
        return;
    };
    let bounds = bevy_winit::WINIT_WINDOWS.with(|all_windows| {
        let all_windows = all_windows.borrow();
        all_windows.get_window(entity).map(|window| {
            let size = window.inner_size();
            let scale = window.scale_factor();
            UiBounds {
                x: 0,
                y: 0,
                width: (size.width as f64 / scale).round() as u32,
                height: (size.height as f64 / scale).round() as u32,
            }
        })
    });
    let Some(bounds) = bounds else {
        return;
    };
    if let Some(surface) = world
        .non_send_mut::<UiNavigationExecutor>()
        .recovery
        .as_mut()
    {
        let _ = surface.webview.set_bounds(webview_rect(bounds));
        return;
    }
    let actions = Arc::new(Mutex::new(Vec::new()));
    let ipc_actions = Arc::clone(&actions);
    let parent_hwnd = Arc::new(std::sync::atomic::AtomicIsize::new(0));
    let ipc_parent_hwnd = Arc::clone(&parent_hwnd);
    let event_loop_proxy =
        std::ops::Deref::deref(world.resource::<bevy_winit::EventLoopProxyWrapper>()).clone();
    let result = bevy_winit::WINIT_WINDOWS.with(|all_windows| {
        let all_windows = all_windows.borrow();
        let window = all_windows
            .get_window(entity)
            .ok_or_else(|| "primary native window is not ready".to_string())?;
        wry::WebViewBuilder::new()
            .with_bounds(webview_rect(bounds))
            .with_transparent(false)
            .with_html(RECOVERY_HTML)
            .with_ipc_handler(move |message| {
                let action = serde_json::from_str::<Value>(message.body())
                    .ok()
                    .and_then(|value| value.get("recovery_action")?.as_str().map(str::to_owned))
                    .and_then(|action| match action.as_str() {
                        "retry" => Some(RecoveryAction::Retry),
                        "disconnect" => Some(RecoveryAction::Disconnect),
                        "quit" => Some(RecoveryAction::Quit),
                        _ => None,
                    });
                if let Some(action) = action {
                    ipc_actions
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(action);
                    invalidate_parent_window(&ipc_parent_hwnd);
                    let _ = event_loop_proxy.send_event(bevy_winit::WinitUserEvent::WakeUp);
                }
            })
            .build_as_child(&**window)
            .map_err(|error| error.to_string())
    });
    match result {
        Ok(webview) => {
            use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};
            use wry::WebViewExtWindows;
            let host_window = unsafe { GetAncestor(webview.hwnd(), GA_ROOT) };
            parent_hwnd.store(host_window.0 as isize, std::sync::atomic::Ordering::Release);
            let mut executor = world.non_send_mut::<UiNavigationExecutor>();
            executor.recovery = Some(RecoveryOverlay { webview, actions });
            if executor.finish_root_presentation_handoff(true) {
                log::info!("Recovery Surface is visible; released retained Root presentations");
            }
        }
        Err(error) => {
            log::error!("cannot create host Recovery Surface: {error}");
            let _ = world.write_message(bevy::app::AppExit::error());
        }
    }
}

fn frame_navigation_allowed(definition: &str, url: &str) -> bool {
    url == "about:blank" || same_definition_path(definition, url).is_some()
}

fn same_definition_path(definition: &str, url: &str) -> Option<String> {
    let custom = format!("roundo-ui://resource/{definition}/");
    let webview2 = format!("http://roundo-ui.resource/{definition}/");
    let path = url
        .strip_prefix(&custom)
        .or_else(|| url.strip_prefix(&webview2))?;
    Some(
        path.split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_string(),
    )
}

#[cfg(target_os = "windows")]
fn sync_committed_navigation(
    mut state: bevy::ecs::system::ResMut<UiLifecycleManager>,
    mut executor: bevy::ecs::system::NonSendMut<UiNavigationExecutor>,
) {
    let updates = executor
        .committed
        .iter_mut()
        .map(|(id, overlay)| {
            let mut load = overlay
                .load_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let focus = std::mem::take(&mut load.focus_requested);
            (*id, load.finished_url.take(), focus)
        })
        .collect::<Vec<_>>();
    for (id, url, focus) in updates {
        if focus {
            let _ = state.focus(id);
        }
        let Some(url) = url else {
            continue;
        };
        let Some(definition) = state
            .instance(id)
            .map(|instance| instance.definition.clone())
        else {
            continue;
        };
        if let Some(path) = same_definition_path(&definition, &url) {
            if let Err(error) = state.navigate_same_definition(id, &path) {
                log::warn!(
                    "cannot update same-definition path for {}: {error}",
                    id.get()
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn webview_rect(bounds: UiBounds) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(bounds.x, bounds.y).into(),
        size: wry::dpi::LogicalSize::new(bounds.width, bounds.height).into(),
    }
}

/// Wry owns child-HWND sizing details; the host only supplies the Bevy client
/// area, including DPI-adjusted logical dimensions.
#[cfg(target_os = "windows")]
fn resize_webview(
    windows: bevy::prelude::Query<
        &bevy::window::Window,
        (
            bevy::ecs::query::With<bevy::window::PrimaryWindow>,
            bevy::ecs::query::Changed<bevy::window::Window>,
        ),
    >,
    mut state: bevy::ecs::system::ResMut<UiLifecycleManager>,
) {
    let Some(window) = windows.iter().next() else {
        return;
    };
    // `apply_windows_input_mode` is the single physical-bounds writer and
    // applies this logical change once, rather than once here and every frame.
    state.set_client_bounds(window.width() as u32, window.height() as u32);
}

/// Makes the persistent child HWND either own input or click through to Bevy.
/// The Win32 detail remains inside this Wry adapter rather than leaking through
/// the UI registry seam.
#[cfg(target_os = "windows")]
fn apply_windows_input_mode(
    state: bevy::ecs::system::Res<UiLifecycleManager>,
    mut executor: Option<bevy::ecs::system::NonSendMut<UiNavigationExecutor>>,
) {
    let Some(executor) = executor.as_deref_mut() else {
        return;
    };
    let focused = state.focused_instance();
    let mut replacement_visible = false;
    for id in state.stacking_order() {
        let Some(instance) = state.instance(id) else {
            continue;
        };
        let Some(resource) = state.registry.resource(&instance.definition) else {
            continue;
        };
        let Some(overlay) = executor.committed.get_mut(&id) else {
            continue;
        };
        let mut presentation_changed = false;
        if overlay.last_bounds != Some(instance.bounds) {
            if let Err(error) = overlay.webview.set_bounds(webview_rect(instance.bounds)) {
                log::warn!(
                    "cannot apply Web UI bounds for instance {}: {error}",
                    id.get()
                );
            } else {
                overlay.last_bounds = Some(instance.bounds);
                presentation_changed = true;
            }
        }
        let mut should_raise = false;
        if overlay.last_visible != instance.visible {
            if let Err(error) = overlay.webview.set_visible(instance.visible) {
                log::error!(
                    "cannot set Web UI visibility for instance {} to {}: {error}",
                    id.get(),
                    instance.visible
                );
            } else {
                log::debug!(
                    "applied Web UI visibility for instance {}: {}",
                    id.get(),
                    instance.visible
                );
                dispatch_lifecycle_event(
                    &overlay.webview,
                    "roundo:visibility",
                    if instance.visible {
                        "visible"
                    } else {
                        "hidden"
                    },
                );
                overlay.last_visible = instance.visible;
                should_raise |= instance.visible;
                presentation_changed = true;
            }
        }
        replacement_visible |= instance.visible && overlay.last_visible;
        if overlay.last_interaction_mode != Some(resource.interaction_mode) {
            apply_windows_webview_mode(overlay, resource.interaction_mode);
            overlay.last_interaction_mode = Some(resource.interaction_mode);
            presentation_changed = true;
        }
        let is_focused = focused == Some(id) && instance.visible;
        if overlay.last_focused != is_focused {
            dispatch_lifecycle_event(
                &overlay.webview,
                "roundo:focus",
                if is_focused { "focused" } else { "blurred" },
            );
            overlay.last_focused = is_focused;
            should_raise |= is_focused;
            presentation_changed = true;
        }
        // Z-order changes only on visibility/focus edges. Raising every child
        // HWND every Bevy frame causes unnecessary WebView2 composition work
        // and can produce visible flashing.
        if should_raise {
            raise_webview(overlay);
        }
        if presentation_changed || should_raise {
            // EventLoopProxy::WakeUp advances Bevy state but does not itself
            // schedule a Win32 paint. Invalidate only on presentation edges so
            // restored/new child WebViews repaint without returning to the old
            // per-IPC invalidation and flicker loop.
            invalidate_webview_parent(&overlay.webview);
        }
    }
    if executor.finish_root_presentation_handoff(replacement_visible) {
        log::info!("Replacement Root is visible; released retained Root presentations");
    }
}

#[cfg(target_os = "windows")]
fn invalidate_parent_window(parent: &std::sync::atomic::AtomicIsize) {
    use windows::Win32::{Foundation::HWND, Graphics::Gdi::InvalidateRect};
    let raw = parent.load(std::sync::atomic::Ordering::Acquire);
    if raw != 0 {
        unsafe {
            let _ = InvalidateRect(Some(HWND(raw as *mut _)), None, false);
        }
    }
}

#[cfg(target_os = "windows")]
fn invalidate_webview_parent(webview: &wry::WebView) {
    use windows::Win32::{
        Graphics::Gdi::InvalidateRect,
        UI::WindowsAndMessaging::{GA_ROOT, GetAncestor},
    };
    use wry::WebViewExtWindows;
    let host_window = unsafe { GetAncestor(webview.hwnd(), GA_ROOT) };
    if !host_window.is_invalid() {
        unsafe {
            let _ = InvalidateRect(Some(host_window), None, false);
        }
    }
}

#[cfg(target_os = "windows")]
fn dispatch_lifecycle_event(webview: &wry::WebView, name: &str, state: &str) {
    let name = serde_json::to_string(name).expect("event name serializes");
    let state = serde_json::to_string(state).expect("event state serializes");
    let script =
        format!("window.dispatchEvent(new CustomEvent({name},{{detail:{{state:{state}}}}}));");
    if let Err(error) = webview.evaluate_script(&script) {
        log::warn!("cannot deliver Web UI lifecycle event: {error}");
    }
}

#[cfg(target_os = "windows")]
fn raise_webview(overlay: &WebViewOverlay) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos};
    use wry::WebViewExtWindows;
    unsafe {
        let _ = SetWindowPos(
            overlay.webview.hwnd(),
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_webview_mode(overlay: &WebViewOverlay, mode: ClientInteractionMode) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE,
        SetWindowLongW, SetWindowPos, WS_DISABLED, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
    };
    use wry::WebViewExtWindows;
    let hwnd = overlay.webview.hwnd();
    // SAFETY: hwnd is the live child HWND owned by this persistent WebView;
    // we only replace its extended style bits on the Bevy main thread.
    unsafe {
        let old_exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let transparent = (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) as i32;
        let exstyle = if mode == ClientInteractionMode::InGame {
            old_exstyle | transparent
        } else {
            old_exstyle & !transparent
        };
        let old_style = GetWindowLongW(hwnd, GWL_STYLE);
        let disabled = WS_DISABLED.0 as i32;
        let style = if mode == ClientInteractionMode::InGame {
            // WS_EX_TRANSPARENT affects painting order, not hit testing. The
            // disabled child must not consume mouse motion intended for Bevy.
            old_style | disabled
        } else {
            old_style & !disabled
        };
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, exstyle);
        let _ = SetWindowLongW(hwnd, GWL_STYLE, style);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
    if mode == ClientInteractionMode::InGame {
        if let Err(error) = overlay.webview.focus_parent() {
            log::error!("cannot return input focus to the game window: {error}");
        }
    }
    // A Web UI child window receives focus normally when clicked. Calling
    // WebView2 MoveFocus while it is replacing a document can fail with
    // E_INVALIDARG and causes a focus/repaint loop, so never force focus here.
}

fn enqueue_webui_command(
    io: &Option<JsonRequestResponseIo<Value>>,
    pending: &Arc<Mutex<Vec<PendingCommand>>>,
    source: Option<UiInstanceId>,
    body: &str,
) {
    let transport: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(error) => {
            log::warn!("invalid Web UI IPC: {error}");
            return;
        }
    };
    let Some(request_id) = transport.get("request_id").and_then(Value::as_u64) else {
        log::warn!("Web UI IPC was rejected: request_id is missing or not an unsigned integer");
        return;
    };
    let Some(mut command) = transport.get("command").cloned() else {
        log::warn!("Web UI IPC request {request_id} was rejected: command is missing");
        pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(PendingCommand {
                request_id,
                command_name: String::new(),
                submitted_at: Instant::now(),
                timeout_logged: false,
                call: None,
                immediate: Some(json!({
                    "version": 1,
                    "command": "",
                    "ok": false,
                    "error": {
                        "code": "invalid_command_envelope",
                        "message": "transport command is required",
                    },
                })),
            });
        return;
    };
    if let (Some(source), Some(object)) = (source, command.as_object_mut()) {
        object.insert("_roundo_source_instance".into(), json!(source.get()));
    }
    let command_name = command
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if command_name == "ui.open" {
        log::debug!(
            "Web UI IPC request {request_id}: enqueueing ui.open with arguments {:?}",
            command.get("arguments")
        );
    }
    let immediate_error = |code: &str, message: &str| json!({"version":1,"command":command_name,"ok":false,"error":{"code":code,"message":message}});
    let submitted_at = Instant::now();
    let item = match io {
        Some(io) => match io.submit(command) {
            Ok(call) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: Some(call),
                immediate: None,
            },
            Err(JsonSubmitError::Full) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: None,
                immediate: Some(immediate_error(
                    "command_queue_full",
                    "command queue is full",
                )),
            },
            Err(JsonSubmitError::Disconnected) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: None,
                immediate: Some(immediate_error(
                    "internal_command_error",
                    "command endpoint disconnected",
                )),
            },
            Err(JsonSubmitError::InputTooLarge { command }) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: None,
                immediate: Some(json!({
                    "version": 1,
                    "command": command,
                    "ok": false,
                    "error": {
                        "code": "command_input_too_large",
                        "message": "command input exceeds 64 KiB",
                    },
                })),
            },
        },
        None => PendingCommand {
            request_id,
            command_name: command_name.clone(),
            submitted_at,
            timeout_logged: false,
            call: None,
            immediate: Some(immediate_error(
                "internal_command_error",
                "command endpoint is unavailable",
            )),
        },
    };
    log::trace!("Web UI IPC request {request_id}: transport submission completed");
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(item);
    log::trace!("Web UI IPC request {request_id}: queued for response polling");
}

#[cfg(target_os = "windows")]
fn resolve_webui_commands(
    mut executor: Option<bevy::ecs::system::NonSendMut<UiNavigationExecutor>>,
) {
    let Some(executor) = executor.as_deref_mut() else {
        return;
    };
    for overlay in executor.committed.values_mut() {
        let mut pending = overlay
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut index = 0;
        while index < pending.len() {
            let result = pending[index].immediate.take().or_else(|| {
                pending[index]
                    .call
                    .as_ref()
                    .and_then(RequestCall::try_result)
            });
            let Some(result) = result else {
                if !pending[index].timeout_logged
                    && pending[index].submitted_at.elapsed() >= Duration::from_secs(2)
                {
                    log::warn!(
                        "Web UI IPC request {} ({}) has waited over two seconds for a client-command response",
                        pending[index].request_id,
                        pending[index].command_name
                    );
                    pending[index].timeout_logged = true;
                }
                index += 1;
                continue;
            };
            let request_id = pending[index].request_id;
            let command_name = &pending[index].command_name;
            if result.get("ok").and_then(Value::as_bool) == Some(false) {
                log::warn!("Web UI IPC request {request_id} ({command_name}) failed: {result}");
            } else if command_name == "ui.open" {
                log::debug!("Web UI IPC request {request_id}: ui.open completed with {result}");
            }
            let script = response_script(request_id, &result);
            if let Err(error) = overlay.webview.evaluate_script(&script) {
                log::error!("cannot resolve Web UI command: {error}");
            }
            pending.swap_remove(index);
        }
    }
}

fn bridge_focus_request(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("roundo_focus_request").and_then(Value::as_bool))
        == Some(true)
}

fn bridge_handshake(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("roundo_bridge_ready").and_then(Value::as_bool))
        == Some(true)
}

fn response_script(request_id: u64, result: &Value) -> String {
    format!(
        "window.__roundoResolve({}, {});",
        request_id,
        serde_json::to_string(result).expect("JSON result is serializable")
    )
}

#[cfg(target_os = "windows")]
const WEBUI_BRIDGE_SCRIPT: &str = r#"(() => { let next = 1; const pending = new Map(); window.__roundoResolve = (id, value) => { const resolve = pending.get(id); if (resolve) { pending.delete(id); resolve(value); } }; window.roundo = { execute(command) { return new Promise(resolve => { const id = next++; pending.set(id, resolve); window.ipc.postMessage(JSON.stringify({ request_id: id, command })); }); } }; window.addEventListener('pointerdown',()=>window.ipc.postMessage(JSON.stringify({roundo_focus_request:true})),true); window.ipc.postMessage(JSON.stringify({roundo_bridge_ready:true})); })();"#;

fn webui_initialization_script(definition: &str) -> String {
    let prefix = serde_json::to_string(&format!("/{definition}/"))
        .expect("Definition path prefix serializes");
    format!(
        "{WEBUI_BRIDGE_SCRIPT}\n(() => {{ const prefix={prefix}; const allowed=(value)=>{{ try {{ const url=new URL(value,location.href); return url.href==='about:blank'||(url.origin===location.origin&&url.pathname.startsWith(prefix)); }} catch (_) {{ return false; }} }}; const check=(frame)=>{{ const value=frame.getAttribute('src'); if(value&&!allowed(value)){{ console.error('Roundo blocked cross-Definition iframe',value); frame.removeAttribute('src'); }} }}; new MutationObserver(records=>{{ for(const record of records){{ if(record.target instanceof HTMLIFrameElement)check(record.target); for(const node of record.addedNodes){{ if(node instanceof HTMLIFrameElement)check(node); if(node.querySelectorAll)node.querySelectorAll('iframe').forEach(check); }} }} }}).observe(document,{{subtree:true,childList:true,attributes:true,attributeFilter:['src']}}); window.addEventListener('DOMContentLoaded',()=>document.querySelectorAll('iframe').forEach(check),{{once:true}}); }})();"
    )
}

#[cfg(target_os = "windows")]
fn protocol_response(
    registry: &UiRegistry,
    source_resource: Option<&str>,
    uri: &wry::http::Uri,
) -> wry::http::Response<Cow<'static, [u8]>> {
    let authority = uri
        .authority()
        .map(|value| value.as_str())
        .unwrap_or_default();
    log::debug!("Web UI custom-protocol request: uri=`{uri}`, source_resource={source_resource:?}");
    match registry.read_protocol_asset(source_resource, authority, uri.path()) {
        Ok((body, mime)) => {
            log::debug!(
                "Web UI custom-protocol response: uri=`{uri}`, status=200, mime={mime}, bytes={}",
                body.len()
            );
            wry::http::Response::builder()
                .header("Content-Type", mime)
                .body(Cow::Owned(body))
                .expect("valid HTTP response")
        }
        Err(error) => {
            log::warn!("Web UI custom-protocol response: uri=`{uri}`, status=404, error={error}");
            wry::http::Response::builder()
                .status(404)
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Cow::Owned(error.to_string().into_bytes()))
                .expect("valid HTTP response")
        }
    }
}

#[derive(Deserialize)]
struct RegistryFile {
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
enum LayoutRegistration {
    Fullscreen,
    Windowed,
}
impl Default for LayoutRegistration {
    fn default() -> Self {
        Self::Fullscreen
    }
}
impl LayoutRegistration {
    fn into_layout(self, width: Option<u32>, height: Option<u32>) -> Result<UiLayout, String> {
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
fn checked_file(root: &Path, relative: &str) -> Result<PathBuf, UiRegistryError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "roundo-webui-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mod_root = root.join("vanilla_ui");
        fs::create_dir_all(mod_root.join("assets/webui/main")).unwrap();
        fs::write(
            mod_root.join("manifest.toml"),
            "[general]\nmod_name='vanilla_ui'\nauthor='vanilla'\n",
        )
        .unwrap();
        fs::write(mod_root.join("assets/webui/main/index.html"), "ok").unwrap();
        fs::write(mod_root.join("assets/webui/main/about.html"), "about").unwrap();
        fs::write(mod_root.join("assets/webui/registry.toml"), "[[ui]]\nname='main'\nproject='main'\nentry='index.html'\ninteraction_mode='web-ui'\nworld_visibility='hidden'\nmax_instances=2\nprefetch=['main']\n[slots]\n'roundo.disconnected-root'='main'
'roundo.main-menu'='main'\n").unwrap();
        root
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn root_replacement_keeps_the_old_root_live_until_the_new_root_commits() {
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut manager = UiLifecycleManager::new(registry);
        manager
            .registry
            .slots
            .insert(CONNECTED_ROOT_SLOT.into(), "vanilla.vanilla_ui.main".into());
        let old_root = manager.open_configured_root().unwrap().unwrap();
        let obsolete_target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
            .unwrap();
        let obsolete_pending = manager
            .begin_open(UiCommandSource::WebView(old_root), obsolete_target)
            .unwrap();
        let mut executor = UiNavigationExecutor::default();

        let pending = match executor
            .replace_root(&mut manager, UiLifecycleState::Connected)
            .unwrap()
        {
            UiRootReplacement::Pending(pending) => pending,
            UiRootReplacement::Completed(_) => panic!("configured Root must load before commit"),
        };

        assert_eq!(manager.lifecycle_state(), UiLifecycleState::Disconnected);
        assert!(manager.instance(old_root).is_some());
        assert!(manager.command_source_is_live(old_root));
        assert_eq!(
            manager.commit_open(obsolete_pending),
            Err(UiLifecycleError::StaleUiInstance),
            "a pending open owned by the old Root must not race its replacement"
        );

        let committed = manager.commit_root_replacement(pending.get()).unwrap();
        assert_eq!(manager.lifecycle_state(), UiLifecycleState::Connected);
        assert_eq!(committed.destroyed, vec![old_root]);
        assert!(manager.instance(old_root).is_none());
        assert!(manager.instance(committed.instance).is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_resolves_definition_prefetch_edges() {
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        assert_eq!(
            registry
                .resource("vanilla.vanilla_ui.main")
                .unwrap()
                .prefetch,
            vec!["vanilla.vanilla_ui.main"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prefetched_document_is_not_live_until_claimed_and_committed() {
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut manager = UiLifecycleManager::new(registry);
        let owner = manager.open_configured_root().unwrap().unwrap();
        let target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
            .unwrap();

        let pending = manager.begin_prefetch(target.clone()).unwrap();
        assert!(manager.pending_descriptor(pending).unwrap().prefetched);
        assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 1);
        assert_eq!(manager.parent_instance(owner), None);

        assert_eq!(
            manager
                .claim_prefetch(UiCommandSource::WebView(owner), &target)
                .unwrap(),
            Some(pending)
        );
        assert!(!manager.pending_descriptor(pending).unwrap().prefetched);
        let instance = manager.commit_open(pending).unwrap();
        assert_eq!(manager.parent_instance(instance), Some(owner));
        assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn staged_command_gate_flushes_in_order_and_never_requeues_after_commit() {
        let mut gate = StagedCommandGate::default();
        assert!(gate.queue_until_commit("first"));
        assert!(gate.queue_until_commit("second"));
        assert_eq!(gate.commit(), vec!["first", "second"]);
        assert!(!gate.queue_until_commit("third"));
        assert!(gate.commit().is_empty());
    }

    #[test]
    fn navigation_rejects_cross_definition_slots_and_external_urls() {
        let source = "owner.mod.main";
        assert!(top_level_navigation_allowed(
            source,
            "roundo-ui://resource/owner.mod.main/about.html"
        ));
        assert!(top_level_navigation_allowed(
            source,
            "http://roundo-ui.resource/owner.mod.main/about.html"
        ));
        assert!(!top_level_navigation_allowed(
            source,
            "roundo-ui://resource/other.mod.page/index.html"
        ));
        assert!(!top_level_navigation_allowed(
            source,
            "roundo-ui://slot/roundo.settings/"
        ));
        assert!(!top_level_navigation_allowed(
            source,
            "https://example.test/"
        ));
    }

    #[test]
    fn frame_navigation_policy_allows_only_same_definition_and_blank() {
        let source = "owner.mod.main";
        assert!(frame_navigation_allowed(source, "about:blank"));
        assert!(frame_navigation_allowed(
            source,
            "http://roundo-ui.resource/owner.mod.main/frame.html"
        ));
        assert!(!frame_navigation_allowed(
            source,
            "http://roundo-ui.resource/other.mod.page/frame.html"
        ));
        assert!(!frame_navigation_allowed(source, "https://example.test/"));
    }

    #[test]
    fn initialization_script_installs_cross_definition_iframe_guard() {
        let script = webui_initialization_script("owner.mod.main");
        assert!(script.contains("MutationObserver"));
        assert!(script.contains("HTMLIFrameElement"));
        assert!(script.contains("/owner.mod.main/"));
        assert!(script.contains("roundo_bridge_ready"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn staged_commit_requires_native_success_and_bridge_and_surfaces_failure_first() {
        let timeout = Duration::from_secs(15);
        assert_eq!(
            staged_readiness(true, None, true, Duration::ZERO, timeout),
            StagedReadiness::Pending
        );
        assert_eq!(
            staged_readiness(true, Some(&Ok(())), false, Duration::from_secs(1), timeout),
            StagedReadiness::Pending
        );
        assert_eq!(
            staged_readiness(true, Some(&Ok(())), true, Duration::from_secs(1), timeout),
            StagedReadiness::Commit
        );
        assert_eq!(
            staged_readiness(
                true,
                Some(&Err("dns".into())),
                true,
                Duration::from_secs(20),
                timeout
            ),
            StagedReadiness::Failed("dns".into())
        );
        assert_eq!(
            staged_readiness(true, None, false, timeout, timeout),
            StagedReadiness::Timeout
        );
        assert_eq!(
            staged_readiness(false, Some(&Ok(())), true, Duration::ZERO, timeout),
            StagedReadiness::Stale
        );
    }

    #[test]
    fn same_definition_navigation_extracts_only_a_local_path() {
        assert_eq!(
            same_definition_path(
                "owner.mod.main",
                "http://roundo-ui.resource/owner.mod.main/settings/page.html?x=1#part"
            ),
            Some("settings/page.html".into())
        );
        assert_eq!(
            same_definition_path(
                "owner.mod.main",
                "roundo-ui://resource/other.mod.page/index.html"
            ),
            None
        );
    }

    #[test]
    fn bridge_handshake_is_explicit_and_cannot_be_confused_with_a_command() {
        assert!(bridge_handshake(r#"{"roundo_bridge_ready":true}"#));
        assert!(bridge_focus_request(r#"{"roundo_focus_request":true}"#));
        assert!(!bridge_focus_request(r#"{"roundo_bridge_ready":true}"#));
        assert!(!bridge_handshake(
            r#"{"request_id":1,"command":{"command":"app.quit"}}"#
        ));
        assert!(!bridge_handshake("not json"));
    }

    #[test]
    fn response_transport_serializes_json_without_script_interpolation() {
        let script = response_script(7, &json!({"ok": true, "message": "done"}));
        assert_eq!(
            script,
            "window.__roundoResolve(7, {\"message\":\"done\",\"ok\":true});"
        );
    }

    #[test]
    fn ipc_submits_the_inner_command_without_transport_fields() {
        let pipe =
            roundo_toolbox::request_response_pipe::RequestResponsePipe::<Value, Value>::bounded(1);
        let io = JsonRequestResponseIo::new(pipe.io());
        let pending = Arc::new(Mutex::new(Vec::new()));
        enqueue_webui_command(
            &Some(io),
            &pending,
            None,
            r#"{"request_id":7,"command":{"version":1,"command":"app.quit","arguments":{}}}"#,
        );
        let (command, _reply) = pipe.try_receive().expect("inner command was queued");
        assert_eq!(
            command,
            json!({"version":1,"command":"app.quit","arguments":{}})
        );
        let pending = pending.lock().unwrap();
        assert_eq!(pending[0].request_id, 7);
        assert!(pending[0].call.is_some());
        assert!(pending[0].immediate.is_none());
    }

    #[test]
    fn ipc_source_context_is_host_bound_and_overwrites_page_data() {
        let pipe =
            roundo_toolbox::request_response_pipe::RequestResponsePipe::<Value, Value>::bounded(1);
        let io = JsonRequestResponseIo::new(pipe.io());
        let pending = Arc::new(Mutex::new(Vec::new()));
        enqueue_webui_command(
            &Some(io),
            &pending,
            Some(UiInstanceId(9)),
            r#"{"request_id":7,"command":{"version":1,"command":"ui.back","arguments":{},"_roundo_source_instance":666}}"#,
        );
        let (command, _) = pipe.try_receive().unwrap();
        assert_eq!(command["_roundo_source_instance"], 9);
        assert!(command["arguments"].get("instance_id").is_none());
    }

    #[test]
    fn ipc_missing_transport_command_becomes_a_typed_immediate_result() {
        let pending = Arc::new(Mutex::new(Vec::new()));
        enqueue_webui_command(&None, &pending, None, r#"{"request_id":7}"#);
        assert_eq!(
            pending.lock().unwrap()[0].immediate,
            Some(json!({
                "version": 1,
                "command": "",
                "ok": false,
                "error": {
                    "code": "invalid_command_envelope",
                    "message": "transport command is required",
                },
            }))
        );
    }

    #[test]
    fn ipc_queue_full_becomes_a_typed_immediate_result() {
        let pipe =
            roundo_toolbox::request_response_pipe::RequestResponsePipe::<Value, Value>::bounded(1);
        let io = JsonRequestResponseIo::new(pipe.io());
        let _held_call = io.submit(json!({"occupied": true})).unwrap();
        let pending = Arc::new(Mutex::new(Vec::new()));
        enqueue_webui_command(
            &Some(io),
            &pending,
            None,
            r#"{"request_id":7,"command":{"version":1,"command":"app.quit","arguments":{}}}"#,
        );
        let pending = pending.lock().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].request_id, 7);
        assert_eq!(
            pending[0].immediate,
            Some(json!({
                "version": 1,
                "command": "app.quit",
                "ok": false,
                "error": {"code": "command_queue_full", "message": "command queue is full"}
            }))
        );
    }

    #[test]
    fn ipc_rejects_oversized_commands_before_queueing() {
        let pipe =
            roundo_toolbox::request_response_pipe::RequestResponsePipe::<Value, Value>::bounded(1);
        let io = JsonRequestResponseIo::new(pipe.io());
        let pending = Arc::new(Mutex::new(Vec::new()));
        let body = serde_json::to_string(&json!({
            "request_id": 7,
            "command": {
                "version": 1,
                "command": "app.quit",
                "arguments": {"payload": "x".repeat(64 * 1024)},
            },
        }))
        .unwrap();
        enqueue_webui_command(&Some(io), &pending, None, &body);
        let result = pending.lock().unwrap()[0].immediate.clone().unwrap();
        assert_eq!(result["version"], 1);
        assert_eq!(result["command"], "app.quit");
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "command_input_too_large");
        assert!(pipe.try_receive().is_none());
    }

    #[test]
    fn ipc_unavailable_and_disconnected_errors_keep_the_inner_command_name() {
        let pending = Arc::new(Mutex::new(Vec::new()));
        let body =
            r#"{"request_id":7,"command":{"version":1,"command":"server.list","arguments":{}}}"#;
        enqueue_webui_command(&None, &pending, None, body);
        assert_eq!(
            pending.lock().unwrap()[0].immediate.as_ref().unwrap()["command"],
            "server.list"
        );

        let pipe =
            roundo_toolbox::request_response_pipe::RequestResponsePipe::<Value, Value>::bounded(1);
        let io = JsonRequestResponseIo::new(pipe.io());
        drop(pipe);
        let pending = Arc::new(Mutex::new(Vec::new()));
        enqueue_webui_command(&Some(io), &pending, None, body);
        let result = pending.lock().unwrap()[0].immediate.clone().unwrap();
        assert_eq!(result["version"], 1);
        assert_eq!(result["command"], "server.list");
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "internal_command_error");
    }

    #[test]
    fn custom_protocol_rejects_cross_definition_frames_and_assets() {
        let root = fixture_root();
        let project = fs::canonicalize(root.join("vanilla_ui/assets/webui/main")).unwrap();
        let owner = parse_mod_id("owner.mod").unwrap();
        let allowed = parse_mod_id("allowed.mod").unwrap();
        let denied = parse_mod_id("denied.mod").unwrap();
        let resource = |name: &str, owner: ModId| UiResource {
            name: name.into(),
            owner,
            project_root: project.clone(),
            entry: project.join("index.html"),
            entry_path: "index.html".into(),
            interaction_mode: ClientInteractionMode::WebUi,
            world_visibility: UiWorldVisibility::Hidden,
            max_instances: 2,
            lifecycle_independent: false,
            presentation: PresentationMode::Exclusive,
            layout: UiLayout::Fullscreen,
            prefetch: Vec::new(),
        };
        let mut registry = UiRegistry::default();
        registry.resources.insert(
            "owner.mod.main".into(),
            resource("owner.mod.main", owner.clone()),
        );
        registry.resources.insert(
            "allowed.mod.shared".into(),
            resource("allowed.mod.shared", allowed.clone()),
        );
        registry.resources.insert(
            "denied.mod.shared".into(),
            resource("denied.mod.shared", denied),
        );
        registry
            .slots
            .insert("roundo.shared".into(), "allowed.mod.shared".into());
        assert!(matches!(
            registry.read_protocol_asset(Some("owner.mod.main"), "slot", "/roundo.shared/"),
            Err(UiRegistryError::InvalidReference(_))
        ));
        assert!(matches!(
            registry.read_protocol_asset(
                Some("owner.mod.main"),
                "resource",
                "/allowed.mod.shared/index.html"
            ),
            Err(UiRegistryError::InvalidReference(_))
        ));
        assert_eq!(
            registry
                .read_protocol_asset(
                    Some("owner.mod.main"),
                    "resource",
                    "/owner.mod.main/index.html"
                )
                .unwrap()
                .0,
            b"ok"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_requires_positive_max_instances_and_applies_definition_defaults() {
        assert!(toml::from_str::<RegistryFile>("[[ui]]\nname='x'\nproject='x'\nentry='index.html'\ninteraction_mode='web-ui'\nworld_visibility='hidden'").is_err());
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let definition = registry.resource("vanilla.vanilla_ui.main").unwrap();
        assert_eq!(definition.max_instances, 2);
        assert!(!definition.lifecycle_independent);
        assert_eq!(definition.presentation, PresentationMode::Exclusive);
        assert_eq!(definition.layout, UiLayout::Fullscreen);
        let path = root.join("vanilla_ui/assets/webui/registry.toml");
        let invalid = fs::read_to_string(&path)
            .unwrap()
            .replace("max_instances=2", "max_instances=0");
        fs::write(&path, invalid).unwrap();
        assert!(UiRegistry::load(&LoadedMods::discover(&root).unwrap()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layout_registration_validates_window_dimensions() {
        assert_eq!(
            LayoutRegistration::Windowed
                .into_layout(Some(480), Some(320))
                .unwrap(),
            UiLayout::Windowed {
                initial_width: 480,
                initial_height: 320
            }
        );
        assert!(
            LayoutRegistration::Windowed
                .into_layout(None, Some(320))
                .is_err()
        );
        assert!(
            LayoutRegistration::Windowed
                .into_layout(Some(0), Some(320))
                .is_err()
        );
        assert!(
            LayoutRegistration::Fullscreen
                .into_layout(Some(480), None)
                .is_err()
        );
    }

    #[test]
    fn ignores_mods_without_webui_resources() {
        let root = fixture_root();
        let model_mod = root.join("vanilla_models");
        fs::create_dir_all(model_mod.join("assets")).unwrap();
        fs::write(
            model_mod.join("manifest.toml"),
            "[general]\nmod_name='vanilla_models'\nauthor='vanilla'\n",
        )
        .unwrap();

        let mods = LoadedMods::discover(&root).unwrap();
        let registry = UiRegistry::load(&mods).unwrap();

        assert_eq!(
            registry.slot(DISCONNECTED_ROOT_SLOT).unwrap().name,
            "vanilla.vanilla_ui.main"
        );
        assert!(registry.resource("vanilla.vanilla_models.main").is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn webview_resource_transactions_are_serial_and_retirement_gets_a_safe_tick() {
        let mut cooldown = false;

        assert!(!claim_webview_creation_turn(1, 0, &mut cooldown));
        assert!(!claim_webview_creation_turn(0, 1, &mut cooldown));
        cooldown = true;
        assert!(!claim_webview_creation_turn(0, 0, &mut cooldown));
        assert!(claim_webview_creation_turn(0, 0, &mut cooldown));
    }

    #[test]
    fn connecting_page_closes_itself_when_the_connection_has_already_completed() {
        let connecting =
            include_str!("../../../mods/vanilla_ui/assets/webui/connecting/index.html");

        assert!(connecting.contains("const connectionStatus=s.status?.status"));
        assert!(connecting.contains("connectionStatus==='connected'"));
        assert!(connecting.contains("command:'ui.back'"));
        assert!(connecting.contains("<main hidden>"));
        assert!(connecting.contains("setInterval(status,1000)"));
        assert!(!connecting.contains("setInterval(status,250)"));
        assert!(!connecting.contains("roundo.hud"));
        assert!(!connecting.contains("command:'ui.open'"));
    }

    #[test]
    fn server_selection_polling_is_serial_and_renders_only_changed_results() {
        let selection =
            include_str!("../../../mods/vanilla_ui/assets/webui/server-selection/index.html");

        assert!(selection.contains("if (loading) return"));
        assert!(selection.contains("finally { loading = false; }"));
        assert!(selection.contains("if (renderedServers === nextServers) return"));
        assert!(selection.contains("setInterval(load, 1000)"));
        assert!(!selection.contains("setInterval(load,250)"));
    }

    #[test]
    fn main_menu_does_not_retain_a_command_lock_while_an_exclusive_child_is_open() {
        let main_menu = include_str!("../../../mods/vanilla_ui/assets/webui/main-menu/index.html");

        assert!(!main_menu.contains("commandPending"));
        assert!(!main_menu.contains("button.disabled"));
    }

    #[test]
    fn vanilla_settings_asset_consumes_command_metadata_and_keeps_edits_atomic() {
        let settings = include_str!("../../../mods/vanilla_ui/assets/webui/settings/index.html");

        assert!(settings.contains("command('bindings.list',{})"));
        assert!(settings.contains("bindingModel.supported_keys"));
        assert!(settings.contains("bindingModel.supported_actions"));
        assert!(!settings.contains("const actions=['"));
        assert!(!settings.contains("const keys=['"));
        assert!(settings.contains("command('bindings.replace'"));
        assert!(settings.contains("old_binding:bindingDraft.old_binding"));
        assert!(settings.contains("bindingDraft.old_binding?'edit':'add'"));
        assert!(settings.contains("cancel.addEventListener('click'"));
        assert!(settings.contains("command('bindings.unbind'"));
        assert!(settings.contains("error.code||'command_error'"));
        assert!(settings.contains("slider.min=def.min"));
        assert!(settings.contains("number.step=def.step"));
        assert!(settings.contains("saveSetting(def,def.default)"));
    }

    #[test]
    fn registry_resolves_initial_and_refuses_project_escape() {
        let root = fixture_root();
        let mods = LoadedMods::discover(&root).unwrap();
        let registry = UiRegistry::load(&mods).unwrap();
        assert_eq!(
            registry.slot(DISCONNECTED_ROOT_SLOT).unwrap().name,
            "vanilla.vanilla_ui.main"
        );
        assert_eq!(
            registry
                .read_asset("vanilla.vanilla_ui.main", "index.html")
                .unwrap()
                .0,
            b"ok"
        );
        assert!(
            registry
                .read_asset("vanilla.vanilla_ui.main", "../manifest.toml")
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn manager_creates_distinct_children_enforces_cap_and_back_is_source_bound() {
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut manager = UiLifecycleManager::new(registry);
        let root_ui = manager.open_configured_root().unwrap().unwrap();
        let target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
            .unwrap();
        let child = manager
            .open(UiCommandSource::WebView(root_ui), target)
            .unwrap();
        assert_ne!(root_ui, child);
        assert_eq!(manager.parent_instance(child), Some(root_ui));
        assert_eq!(manager.parent_instance(root_ui), None);
        assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 2);
        assert_eq!(manager.adapter_count(), 2);
        let target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", None)
            .unwrap();
        assert_eq!(
            manager.open(UiCommandSource::Host, target),
            Err(UiLifecycleError::UiInstanceLimit)
        );
        assert_eq!(manager.back(child).unwrap(), vec![child]);
        assert!(manager.instance(root_ui).is_some());
        assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 1);
        assert_eq!(manager.adapter_count(), 1);
        assert_eq!(manager.back(child), Err(UiLifecycleError::StaleUiInstance));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_pending_open_is_rejected_but_back_allows_a_fresh_instance() {
        let root = fixture_root();
        let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut settings = registry
            .resource("vanilla.vanilla_ui.main")
            .unwrap()
            .clone();
        settings.name = "vanilla.vanilla_ui.settings".into();
        settings.max_instances = 1;
        registry.resources.insert(settings.name.clone(), settings);
        let mut manager = UiLifecycleManager::new(registry);
        let root_ui = manager.open_configured_root().unwrap().unwrap();
        let target = manager
            .resolve_resource_path("vanilla.vanilla_ui.settings", None)
            .unwrap();

        let pending = manager
            .begin_open(UiCommandSource::WebView(root_ui), target.clone())
            .unwrap();
        assert_eq!(
            manager.begin_open(UiCommandSource::WebView(root_ui), target.clone()),
            Err(UiLifecycleError::DuplicatePendingOpen)
        );
        let first = manager.commit_open(pending).unwrap();
        assert_eq!(manager.live_count("vanilla.vanilla_ui.settings"), 1);

        assert_eq!(manager.back(first).unwrap(), vec![first]);
        assert_eq!(manager.live_count("vanilla.vanilla_ui.settings"), 0);
        let second = manager
            .open(UiCommandSource::WebView(root_ui), target)
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(manager.live_count("vanilla.vanilla_ui.settings"), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn presentation_hides_exclusive_branches_without_unloading_and_restores_focus() {
        let root = fixture_root();
        let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let base = registry
            .resource("vanilla.vanilla_ui.main")
            .unwrap()
            .clone();
        let mut exclusive = base.clone();
        exclusive.name = "vanilla.vanilla_ui.exclusive".into();
        exclusive.max_instances = 4;
        exclusive.presentation = PresentationMode::Exclusive;
        let mut concurrent = base;
        concurrent.name = "vanilla.vanilla_ui.concurrent".into();
        concurrent.max_instances = 4;
        concurrent.presentation = PresentationMode::Concurrent;
        registry.resources.insert(exclusive.name.clone(), exclusive);
        registry
            .resources
            .insert(concurrent.name.clone(), concurrent);
        let mut manager = UiLifecycleManager::new(registry);
        let parent = manager.open_configured_root().unwrap().unwrap();
        let first = manager
            .open_resource(
                UiCommandSource::WebView(parent),
                "vanilla.vanilla_ui.exclusive",
                None,
            )
            .unwrap();
        let second = manager
            .open_resource(
                UiCommandSource::WebView(parent),
                "vanilla.vanilla_ui.exclusive",
                None,
            )
            .unwrap();
        assert!(!manager.instance(parent).unwrap().visible);
        assert!(!manager.instance(first).unwrap().visible);
        assert!(manager.instance(second).unwrap().visible);
        assert!(manager.instance(first).unwrap().loaded);
        assert!(manager.command_source_is_live(first));
        manager.back(second).unwrap();
        assert!(manager.instance(first).unwrap().visible);
        assert!(manager.instance(first).unwrap().loaded);
        manager.back(first).unwrap();
        let sibling = manager
            .open_resource(
                UiCommandSource::WebView(parent),
                "vanilla.vanilla_ui.concurrent",
                None,
            )
            .unwrap();
        let sibling_two = manager
            .open_resource(
                UiCommandSource::WebView(parent),
                "vanilla.vanilla_ui.concurrent",
                None,
            )
            .unwrap();
        assert!(manager.instance(sibling).unwrap().visible);
        assert!(manager.instance(sibling_two).unwrap().visible);
        assert_eq!(
            manager.interactive_instance_at(10.0, 10.0),
            Some(sibling_two)
        );
        manager.focus(parent).unwrap();
        assert_eq!(
            manager.stacking_order(),
            vec![parent, sibling, sibling_two],
            "a focused ancestor must remain physically behind concurrent descendants"
        );
        assert_eq!(
            manager.interactive_instance_at(10.0, 10.0),
            Some(sibling_two)
        );
        let nested = manager
            .open_resource(
                UiCommandSource::WebView(sibling),
                "vanilla.vanilla_ui.concurrent",
                None,
            )
            .unwrap();
        assert_eq!(
            manager.stacking_order(),
            vec![parent, sibling_two, sibling, nested],
            "the branch containing the newest focused descendant must rise as a unit"
        );
        assert_eq!(manager.interactive_instance_at(10.0, 10.0), Some(nested));
        manager.focus(sibling_two).unwrap();
        assert_eq!(
            manager.stacking_order(),
            vec![parent, sibling, nested, sibling_two]
        );
        assert_eq!(
            manager.interactive_instance_at(10.0, 10.0),
            Some(sibling_two)
        );
        assert_eq!(manager.interactive_instance_at(5000.0, 5000.0), None);
        manager.clear_focus();
        assert_eq!(manager.focused_instance(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windowed_survivor_preserves_geometry_and_adapter_identity_after_back() {
        let root = fixture_root();
        let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut window = registry
            .resource("vanilla.vanilla_ui.main")
            .unwrap()
            .clone();
        window.name = "vanilla.vanilla_ui.window".into();
        window.max_instances = 4;
        window.presentation = PresentationMode::Concurrent;
        window.lifecycle_independent = true;
        window.layout = UiLayout::Windowed {
            initial_width: 480,
            initial_height: 320,
        };
        registry.resources.insert(window.name.clone(), window);
        let mut manager = UiLifecycleManager::new(registry);
        manager.set_client_bounds(1000, 800);
        let parent = manager.open_configured_root().unwrap().unwrap();
        let child = manager
            .open_resource(
                UiCommandSource::WebView(parent),
                "vanilla.vanilla_ui.window",
                None,
            )
            .unwrap();
        let bounds = UiBounds {
            x: 11,
            y: 22,
            width: 333,
            height: 444,
        };
        manager.set_bounds(child, bounds).unwrap();
        let adapter = manager.adapter_identity(child).unwrap();
        manager.back(parent).unwrap();
        assert_eq!(manager.parent_instance(child), None);
        assert_eq!(manager.instance(child).unwrap().bounds, bounds);
        assert_eq!(manager.adapter_identity(child), Some(adapter));
        assert!(manager.instance(child).unwrap().visible);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn definition_unload_destroys_targets_and_reparents_retained_independent_descendants() {
        let root = fixture_root();
        let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut retained = registry
            .resource("vanilla.vanilla_ui.main")
            .unwrap()
            .clone();
        retained.name = "vanilla.vanilla_ui.retained".into();
        retained.lifecycle_independent = true;
        retained.presentation = PresentationMode::Concurrent;
        registry.resources.insert(retained.name.clone(), retained);
        let mut manager = UiLifecycleManager::new(registry);
        let parent = manager.open_configured_root().unwrap().unwrap();
        let survivor = manager
            .open_resource(
                UiCommandSource::WebView(parent),
                "vanilla.vanilla_ui.retained",
                None,
            )
            .unwrap();
        let pending_target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", None)
            .unwrap();
        let pending = manager
            .begin_open(UiCommandSource::WebView(survivor), pending_target)
            .unwrap();

        let destroyed = manager
            .unload_definitions(["vanilla.vanilla_ui.main".to_string()])
            .unwrap();
        assert_eq!(destroyed, vec![parent]);
        assert_eq!(manager.parent_instance(survivor), None);
        assert!(manager.instance(survivor).is_some());
        assert!(
            manager
                .registry
                .resource("vanilla.vanilla_ui.main")
                .is_none()
        );
        assert!(
            manager
                .registry
                .resource("vanilla.vanilla_ui.retained")
                .is_some()
        );
        assert_eq!(
            manager.commit_open(pending),
            Err(UiLifecycleError::StaleUiInstance)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_load_failure_activates_recovery_without_committing_or_counting() {
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut manager = UiLifecycleManager::new(registry);
        let pending = manager.begin_configured_root().unwrap().unwrap();
        manager.fail_open(pending, "bridge failed").unwrap();
        let recovery = manager.recovery_surface().unwrap();
        assert_eq!(recovery.lifecycle, UiLifecycleState::Disconnected);
        assert_eq!(recovery.failed_resource, "vanilla.vanilla_ui.main");
        assert_eq!(manager.live_count("vanilla.vanilla_ui.main"), 0);
        assert_eq!(manager.instances().count(), 0);
        let retry = manager.retry_recovery().unwrap();
        assert!(manager.pending_descriptor(retry).is_some());
        assert!(manager.recovery_surface().is_none());
        manager.replace_root(UiLifecycleState::Connected).unwrap();
        assert!(manager.recovery_surface().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_candidate_survives_root_replacement_until_claimed() {
        let root = fixture_root();
        let registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        let mut manager = UiLifecycleManager::new(registry);
        let target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", Some("about.html"))
            .unwrap();
        let prepared = manager.begin_prefetch(target.clone()).unwrap();

        manager.replace_root(UiLifecycleState::Connected).unwrap();

        assert!(manager.pending_descriptor(prepared).unwrap().prefetched);
        assert_eq!(
            manager
                .claim_prefetch(UiCommandSource::Host, &target)
                .unwrap(),
            Some(prepared)
        );
        assert!(manager.commit_open(prepared).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_open_is_cancelled_by_root_replacement_and_optional_root_is_valid() {
        let root = fixture_root();
        let mut registry = UiRegistry::load(&LoadedMods::discover(&root).unwrap()).unwrap();
        registry.slots.remove(CONNECTED_ROOT_SLOT);
        let mut manager = UiLifecycleManager::new(registry);
        let target = manager
            .resolve_resource_path("vanilla.vanilla_ui.main", None)
            .unwrap();
        let pending = manager.begin_open(UiCommandSource::Host, target).unwrap();
        manager.replace_root(UiLifecycleState::Connected).unwrap();
        assert_eq!(
            manager.commit_open(pending),
            Err(UiLifecycleError::StaleUiInstance)
        );
        assert_eq!(manager.open_configured_root().unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }
}
