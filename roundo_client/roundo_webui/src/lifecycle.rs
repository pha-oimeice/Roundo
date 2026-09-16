//! UI Lifecycle Tree 的纯数据 core。
//!
//! 此 module 是 UI Instance、Pending UI Open 与 Root Replacement 的唯一权威；
//! 平台 adapter 只能通过其 interface 报告物理视图结果。

use crate::registry::checked_file;
use crate::{
    CONNECTED_ROOT_SLOT, ClientInteractionMode, DISCONNECTED_ROOT_SLOT, PresentationMode, UiLayout,
    UiRegistry, UiRegistryError, UiWorldVisibility,
};
use bevy::prelude::{Message, Resource};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct UiInstanceId(u64);
impl UiInstanceId {
    pub fn value(self) -> u64 {
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
    pub(crate) path: String,
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
    /// A physically prepared WebView is not a logical open until a caller
    /// claims it. Its Mod document is loaded only after that claim.
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
/// Host-owned lifecycle authority. Root anchors are never UI instances.
#[derive(Resource, Debug)]
pub struct UiLifecycleManager {
    pub(crate) registry: UiRegistry,
    lifecycle_state: UiLifecycleState,
    root_generation: u64,
    tree: roundo_lifecycle::AnchorTree<u64, Option<String>>,
    instances: BTreeMap<UiInstanceId, UiInstance>,
    live_counts: BTreeMap<String, u32>,
    pending: BTreeMap<u64, PendingUiOpen>,
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
    UnknownImport(String),
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
            Self::UnknownImport(handle) => write!(f, "unknown UI import `{handle}`"),
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
            tree: roundo_lifecycle::AnchorTree::new(0, None),
            instances: BTreeMap::new(),
            live_counts: BTreeMap::new(),
            pending: BTreeMap::new(),
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
    pub(crate) fn instance(&self, id: UiInstanceId) -> Option<&UiInstance> {
        let instance = self.instances.get(&id);
        instance
    }
    #[cfg(test)]
    pub(crate) fn parent_instance(&self, id: UiInstanceId) -> Option<UiInstanceId> {
        self.tree
            .parent(id.0)
            .filter(|parent| *parent != 0)
            .map(UiInstanceId)
    }
    pub(crate) fn instances(&self) -> impl Iterator<Item = &UiInstance> {
        self.instances.values()
    }
    /// Returns physical stacking from back to front. Ownership descendants
    /// always remain above their ancestors; focus z-order only chooses between
    /// sibling subtrees. This prevents a focused fullscreen parent from
    /// visually covering a concurrent windowed child.
    pub(crate) fn stacking_order(&self) -> Vec<UiInstanceId> {
        fn collect_subtree_z_orders(
            manager: &UiLifecycleManager,
            node: u64,
            output: &mut BTreeMap<u64, u64>,
        ) -> u64 {
            let instance = manager.instances.get(&UiInstanceId(node));
            let mut highest = instance.map_or(0, |instance| instance.z_order);
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
                    let branch_z_order = subtree_z_orders.get(&id).copied().unwrap_or_default();
                    (branch_z_order, UiInstanceId(id))
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
    pub(crate) fn live_count(&self, definition: &str) -> u32 {
        let count = self.live_counts.get(definition);
        count.copied().unwrap_or(0)
    }
    pub(crate) fn set_client_bounds(&mut self, width: u32, height: u32) {
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
    #[cfg(test)]
    pub(crate) fn set_bounds(
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
    pub(crate) fn focus(&mut self, id: UiInstanceId) -> Result<(), UiLifecycleError> {
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
            let instance = self.instances.get(id);
            instance.is_some_and(|instance| {
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
        let instance = self.instances.get(&id);
        instance.is_some_and(|instance| instance.loaded)
    }
    pub(crate) fn focused_instance(&self) -> Option<UiInstanceId> {
        self.focus_history.iter().rev().copied().find(|id| {
            let instance = self.instances.get(id);
            instance.is_some_and(|instance| {
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
    /// Resolves a source-local navigation capability. Web UIs never name a
    /// slot, Definition, or target path directly.
    pub fn resolve_import(
        &self,
        source: UiInstanceId,
        handle: &str,
    ) -> Result<UiOpenTarget, UiLifecycleError> {
        if !self.command_source_is_live(source) {
            return Err(UiLifecycleError::StaleUiInstance);
        }
        let source_instance = self.instances.get(&source);
        let definition = &source_instance
            .expect("live command source has an instance")
            .definition;
        let target = self
            .registry
            .resource(definition)
            .and_then(|resource| {
                let import = resource.imports.get(handle);
                import
            })
            .ok_or_else(|| UiLifecycleError::UnknownImport(handle.into()))?;
        self.resolve_resource_path(&target.resource, None)
            .map_err(|error| UiLifecycleError::Registry(error.to_string()))
    }

    pub fn resolve_slot_path(
        &self,
        slot: &str,
        path: Option<&str>,
    ) -> Result<UiOpenTarget, UiRegistryError> {
        let slot_definition = self.registry.slots.get(slot);
        let name = slot_definition.ok_or_else(|| UiRegistryError::UnknownSlot(slot.into()))?;
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
    #[cfg(test)]
    pub(crate) fn open_resource(
        &mut self,
        source: UiCommandSource,
        resource: &str,
        path: Option<&str>,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let target = self
            .resolve_resource_path(resource, path)
            .map_err(|e| UiLifecycleError::Registry(e.to_string()))?;
        self.open_immediately(source, target)
    }
    pub(crate) fn begin_open(
        &mut self,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<u64, UiLifecycleError> {
        self.begin_open_internal(source, target, false, None, false)
    }

    /// Removes speculative candidates outside the current visible import graph.
    /// Claimed Pending UI Opens are never affected.
    pub(crate) fn retain_prefetch_targets(&mut self, desired: &BTreeSet<UiOpenTarget>) -> Vec<u64> {
        let removed = self
            .pending
            .iter()
            .filter_map(|(id, pending)| {
                (pending.prefetched && !desired.contains(&pending.target)).then_some(*id)
            })
            .collect::<Vec<_>>();
        for id in &removed {
            let removed_pending = self.pending.remove(id);
            debug_assert!(
                removed_pending.is_some(),
                "collected pending open must exist"
            );
        }
        removed
    }

    /// Reserves one hidden physical WebView without loading its Mod document,
    /// creating a logical UI instance, or consuming the Definition's live allowance.
    pub(crate) fn begin_prefetch(&mut self, target: UiOpenTarget) -> Result<u64, UiLifecycleError> {
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

    /// Converts a prepared WebView into the caller's real Pending UI Open.
    /// Its future identity is retained, while ownership is assigned only now;
    /// the Mod document may start loading after this transition.
    pub(crate) fn claim_prefetch(
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
    pub(crate) fn pending_descriptor(&self, pending_id: u64) -> Option<PendingUiDescriptor> {
        let pending = self.pending.get(&pending_id);
        pending.map(|pending| PendingUiDescriptor {
            id: pending_id,
            target: pending.target.clone(),
            root_generation: pending.root_generation,
            prefetched: pending.prefetched,
        })
    }
    pub(crate) fn pending_descriptors(&self) -> Vec<PendingUiDescriptor> {
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
    /// Dismisses the host-owned Recovery Surface without selecting or replacing a Root.
    /// The authoritative connection adapter remains solely responsible for Root selection.
    pub fn dismiss_recovery_surface(&mut self) -> bool {
        self.recovery.take().is_some()
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
    pub(crate) fn fail_open(
        &mut self,
        pending_id: u64,
        message: impl Into<String>,
    ) -> Result<(), UiLifecycleError> {
        let pending = self.pending.remove(&pending_id);
        let pending = pending.ok_or(UiLifecycleError::StaleUiInstance)?;
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
    pub(crate) fn commit_open(
        &mut self,
        pending_id: u64,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let pending = self.pending.remove(&pending_id);
        let pending = pending.ok_or(UiLifecycleError::StaleUiInstance)?;
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

    pub(crate) fn pending_root_replacement(&self, pending_id: u64) -> Option<UiLifecycleState> {
        let pending = self.pending.get(&pending_id);
        pending.and_then(|pending| pending.replacement_lifecycle)
    }

    pub fn pending_root_replacement_lifecycle(&self) -> Option<UiLifecycleState> {
        self.pending
            .values()
            .find_map(|pending| pending.replacement_lifecycle)
    }

    pub(crate) fn cancel_root_replacement(&mut self) -> Vec<u64> {
        let cancelled = self
            .pending
            .iter()
            .filter_map(|(id, pending)| pending.replacement_lifecycle.map(|_| *id))
            .collect::<Vec<_>>();
        for id in &cancelled {
            let removed_pending = self.pending.remove(id);
            debug_assert!(
                removed_pending.is_some(),
                "collected replacement must exist"
            );
        }
        cancelled
    }

    pub(crate) fn commit_root_replacement(
        &mut self,
        pending_id: u64,
    ) -> Result<UiRootCommit, UiLifecycleError> {
        let pending = self.pending.remove(&pending_id);
        let mut pending = pending.ok_or(UiLifecycleError::StaleUiInstance)?;
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

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn open_immediately(
        &mut self,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<UiInstanceId, UiLifecycleError> {
        let pending = self.begin_open(source, target)?;
        self.commit_open(pending)
    }
    pub(crate) fn navigate_same_definition(
        &mut self,
        source: UiInstanceId,
        path: &str,
    ) -> Result<(), UiLifecycleError> {
        let instance = self.instances.get(&source);
        let instance = instance.ok_or(UiLifecycleError::StaleUiInstance)?;
        let target = self
            .resolve_resource_path(&instance.definition, Some(path))
            .map_err(|error| UiLifecycleError::Registry(error.to_string()))?;
        self.instances
            .get_mut(&source)
            .expect("source was validated")
            .current_path = target.path;
        Ok(())
    }
    pub(crate) fn back(
        &mut self,
        source: UiInstanceId,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
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
            .apply_plan(plan)
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
    pub(crate) fn replace_root(
        &mut self,
        state: UiLifecycleState,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let plan = self
            .tree
            .plan_destruction([0], true, |_, _| false)
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let outcome = self
            .tree
            .apply_plan(plan)
            .map_err(|e| UiLifecycleError::InternalTree(format!("{e:?}")))?;
        let destroyed = outcome
            .destroyed
            .into_iter()
            .filter(|v| *v != 0)
            .map(UiInstanceId)
            .collect::<Vec<_>>();
        self.remove_instances(&destroyed);
        // Prepared candidates have no lifecycle parent until claimed, so a
        // Root replacement must not throw away their prepared physical WebViews.
        // Ordinary Pending UI Opens remain owned by the old Root and die here.
        self.pending.retain(|_, pending| pending.prefetched);
        self.recovery = None;
        self.root_generation += 1;
        for pending in self.pending.values_mut() {
            pending.root_generation = self.root_generation;
            pending.source = UiCommandSource::Host;
            pending.parent = 0;
        }
        self.tree = roundo_lifecycle::AnchorTree::new(0, None);
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

    pub(crate) fn begin_root_replacement(
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

    pub(crate) fn begin_configured_root(&mut self) -> Result<Option<u64>, UiLifecycleError> {
        let Some(target) = self.configured_root_target_for(self.lifecycle_state)? else {
            return Ok(None);
        };
        self.begin_open_internal(UiCommandSource::Host, target, true, None, false)
            .map(Some)
    }
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn open_configured_root(
        &mut self,
    ) -> Result<Option<UiInstanceId>, UiLifecycleError> {
        let Some(pending) = self.begin_configured_root()? else {
            return Ok(None);
        };
        self.commit_open(pending).map(Some)
    }

    pub(crate) fn unload_definitions(
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
                .apply_plan(plan)
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
                UiCommandSource::WebView(source) => {
                    let source_instance = self.instances.get(&source);
                    source_instance
                        .is_none_or(|instance| definitions.contains(&instance.definition))
                }
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
            let _removed_count = self.live_counts.remove(definition);
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
                    let instance = self.instances.get(&id);
                    instance.and_then(|instance| {
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
