//! Wry 与 Win32 平台 adapter。
//!
//! 物理 WebView、Win32 输入和 custom protocol 只在此处实现；它们消费
//! `UiLifecycleManager` interface，绝不拥有 UI Lifecycle Tree。

use crate::registry::{MAX_PREPARED_COMMANDS, MAX_PREPARED_UI_CANDIDATES};
use crate::*;
use roundo_toolbox::request_response_pipe::{
    ContextualJsonRequestResponseIo, JsonSubmitError, RequestCall, ResponseSender,
};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
fn webview2_resource_url(resource: &str, path: &str) -> String {
    // Match Wry's Windows custom-protocol workaround. `WebView::load_url`
    // does not apply this conversion (only builder-time URLs do).
    format!("http://roundo-ui.resource/{resource}/{path}")
}

pub(crate) fn top_level_navigation_allowed(source_resource: &str, url: &str) -> bool {
    let custom = format!("roundo-ui://resource/{source_resource}/");
    let webview2 = format!("http://roundo-ui.resource/{source_resource}/");
    url.starts_with(&custom) || url.starts_with(&webview2)
}

#[cfg(target_os = "windows")]
struct WebViewOverlay {
    webview: wry::WebView,
    pending: Arc<Mutex<Vec<PendingCommand>>>,
    subscriptions: Arc<Mutex<BTreeSet<String>>>,
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
pub(crate) enum StagedReadiness {
    Pending,
    Commit,
    Failed(String),
    Timeout,
    Stale,
}

#[cfg(target_os = "windows")]
pub(crate) fn should_start_document_load(prefetched: bool, initial_blank_finished: bool) -> bool {
    !prefetched && initial_blank_finished
}

#[cfg(target_os = "windows")]
pub(crate) fn prepared_webview_readiness(
    pending_live: bool,
    initial_blank_finished: bool,
    elapsed: Duration,
    timeout: Duration,
) -> StagedReadiness {
    if !pending_live {
        StagedReadiness::Stale
    } else if initial_blank_finished {
        StagedReadiness::Commit
    } else if elapsed >= timeout {
        StagedReadiness::Timeout
    } else {
        StagedReadiness::Pending
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn staged_readiness(
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
pub(crate) struct StagedCommandGate {
    admitted: bool,
    queued: Vec<String>,
    overflow_logged: bool,
}

#[cfg(target_os = "windows")]
impl StagedCommandGate {
    pub(crate) fn queue_until_commit(&mut self, body: &str) -> bool {
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

    pub(crate) fn commit(&mut self) -> Vec<String> {
        self.admitted = true;
        std::mem::take(&mut self.queued)
    }
}

#[cfg(target_os = "windows")]
struct StagedWebView {
    webview: wry::WebView,
    target_url: Option<String>,
    pending_commands: Arc<Mutex<Vec<PendingCommand>>>,
    subscriptions: Arc<Mutex<BTreeSet<String>>>,
    command_gate: Arc<Mutex<StagedCommandGate>>,
    command_io: Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
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
    /// Graph-prefetched physical WebViews whose Mod documents are not loaded.
    /// They consume neither a lifecycle instance nor `max_instances` until claimed.
    #[cfg(target_os = "windows")]
    prepared: BTreeMap<u64, StagedWebView>,
    /// Superseded staged WebViews are moved here instead of dropped while a
    /// WebView2 navigation callback may still be on the Win32 stack. Physical
    /// controller creation also observes one creation-free update after COM
    /// teardown or command-response script evaluation, so those operations
    /// cannot share an active WebView2 callback turn.
    #[cfg(target_os = "windows")]
    superseded_staged: BTreeMap<u64, StagedWebView>,
    #[cfg(target_os = "windows")]
    webview_creation_cooldown: bool,
    #[cfg(target_os = "windows")]
    deferred_open_responses: BTreeMap<u64, DeferredOpenResponse>,
    #[cfg(target_os = "windows")]
    recovery: Option<RecoveryOverlay>,
    /// Advances whenever a live WebView changes its Client Data subscription
    /// set. Client-owned publishers use it to send an initial snapshot without
    /// turning subscription control into a source-dependent Client Command.
    #[cfg(target_os = "windows")]
    data_subscription_generation: Arc<std::sync::atomic::AtomicU64>,
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
            #[cfg(target_os = "windows")]
            data_subscription_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}
impl UiNavigationExecutor {
    /// Monotonic signal for Client Data subscription changes. It contains no
    /// data itself; the authoritative client decides what and when to publish.
    pub fn data_subscription_generation(&self) -> u64 {
        #[cfg(target_os = "windows")]
        {
            return self
                .data_subscription_generation
                .load(std::sync::atomic::Ordering::Acquire);
        }
        #[cfg(not(target_os = "windows"))]
        0
    }

    pub fn has_data_subscribers(&self, resource: &str) -> bool {
        #[cfg(target_os = "windows")]
        {
            return self.committed.values().any(|overlay| {
                overlay
                    .subscriptions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(resource)
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = resource;
            false
        }
    }

    /// Pushes one client-owned snapshot only to WebViews subscribed to the
    /// named Client Data resource.
    pub fn publish_data(&mut self, resource: &str, data: &Value) -> usize {
        #[cfg(target_os = "windows")]
        {
            let script = data_sync_script(resource, data);
            let mut delivered = 0;
            for overlay in self.committed.values() {
                let subscribed = overlay
                    .subscriptions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(resource);
                if subscribed {
                    match overlay.webview.evaluate_script(&script) {
                        Ok(()) => delivered += 1,
                        Err(error) => log::error!(
                            "cannot synchronize Client Data resource `{resource}`: {error}"
                        ),
                    }
                }
            }
            if delivered != 0 {
                self.webview_creation_cooldown = true;
            }
            return delivered;
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (resource, data);
            0
        }
    }

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
pub(crate) struct WebUiCommandEndpoint {
    pub(crate) io: Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
}

pub(crate) struct PendingCommand {
    pub(crate) request_id: u64,
    command_name: String,
    submitted_at: Instant,
    timeout_logged: bool,
    pub(crate) call: Option<RequestCall<Value>>,
    pub(crate) immediate: Option<Value>,
}

#[cfg(target_os = "windows")]
pub(crate) fn claim_webview_creation_turn(
    active_transactions: usize,
    retiring_transactions: usize,
    commands_pending: bool,
    cooldown: &mut bool,
) -> bool {
    if active_transactions != 0 || retiring_transactions != 0 || commands_pending {
        return false;
    }
    if std::mem::take(cooldown) {
        return false;
    }
    true
}

#[cfg(target_os = "windows")]
pub(crate) fn retire_superseded_staged_webviews(world: &mut bevy::prelude::World) {
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
pub(crate) fn schedule_graph_prefetches(mut state: bevy::prelude::ResMut<UiLifecycleManager>) {
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
pub(crate) fn stage_pending_webview(world: &mut bevy::prelude::World) {
    let entity = world.query_filtered::<bevy::prelude::Entity, bevy::ecs::query::With<bevy::window::PrimaryWindow>>().iter(world).next();
    let Some(entity) = entity else {
        return;
    };
    let may_create = {
        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
        let active = executor.staged.len();
        let retiring = executor.superseded_staged.len();
        let commands_pending = executor.committed.values().any(|overlay| {
            !overlay
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        });
        let cooldown = &mut executor.webview_creation_cooldown;
        claim_webview_creation_turn(active, retiring, commands_pending, cooldown)
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
    let subscriptions = Arc::new(Mutex::new(BTreeSet::new()));
    let ipc_subscriptions = Arc::clone(&subscriptions);
    let subscription_generation = Arc::clone(
        &world
            .non_send::<UiNavigationExecutor>()
            .data_subscription_generation,
    );
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
                    dispatch_webui_message(
                        &command_io,
                        &ipc_pending_commands,
                        &ipc_subscriptions,
                        &subscription_generation,
                        command_source,
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
                    subscriptions,
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
pub(crate) fn advance_staged_webviews(world: &mut bevy::prelude::World) {
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
            if let Some(mut prepared) = executor.prepared.remove(&id) {
                resume_prepared_webview(&prepared.webview);
                prepared.created_at = Instant::now();
                executor.staged.insert(id, prepared);
            }
        }
        for id in stale {
            executor.supersede_staged_webview(id);
        }
    }

    // Wry starts an asynchronous initial about:blank navigation while building
    // a WebView. A speculative candidate stops there: its Mod document must not
    // load or execute until a real open claims it. Cold and claimed opens wait
    // for about:blank, then navigate with native status handlers installed.
    {
        let prefetched = world
            .resource::<UiLifecycleManager>()
            .pending_descriptors()
            .into_iter()
            .filter(|descriptor| descriptor.prefetched)
            .map(|descriptor| descriptor.id)
            .collect::<BTreeSet<_>>();
        let mut executor = world.non_send_mut::<UiNavigationExecutor>();
        for (id, staged) in &mut executor.staged {
            let initial_finished = staged
                .load_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finished_url
                .as_deref()
                == Some("about:blank");
            if !should_start_document_load(prefetched.contains(id), initial_finished) {
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
                let readiness = if descriptor
                    .as_ref()
                    .is_some_and(|descriptor| descriptor.prefetched)
                {
                    prepared_webview_readiness(
                        true,
                        load.finished_url.as_deref() == Some("about:blank"),
                        staged.created_at.elapsed(),
                        executor.load_timeout,
                    )
                } else {
                    staged_readiness(
                        descriptor.is_some(),
                        load.navigation_result.as_ref(),
                        load.bridge_ready,
                        staged.created_at.elapsed(),
                        executor.load_timeout,
                    )
                };
                match readiness {
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
                            dispatch_webui_message(
                                &staged.command_io,
                                &staged.pending_commands,
                                &staged.subscriptions,
                                &world
                                    .non_send::<UiNavigationExecutor>()
                                    .data_subscription_generation,
                                staged.command_source,
                                &body,
                            );
                        }
                        activate_webui_bridge(&staged.webview);
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
                                subscriptions: staged.subscriptions,
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
pub(crate) fn sync_recovery_surface(world: &mut bevy::prelude::World) {
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
        // The Recovery Surface is a platform adapter: it reports intent but
        // never changes the Lifecycle Tree or connection authority itself.
        let _ = world.write_message(RecoveryActionRequest(action));
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

pub(crate) fn frame_navigation_allowed(definition: &str, url: &str) -> bool {
    url == "about:blank" || same_definition_path(definition, url).is_some()
}

pub(crate) fn same_definition_path(definition: &str, url: &str) -> Option<String> {
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
pub(crate) fn sync_committed_navigation(
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
        if let Some(overlay) = executor.committed.get(&id) {
            activate_webui_bridge(&overlay.webview);
        }
    }
}

#[cfg(target_os = "windows")]
fn activate_webui_bridge(webview: &wry::WebView) {
    if let Err(error) = webview.evaluate_script("window.__roundoActivate();") {
        log::error!("cannot activate committed Web UI bridge: {error}");
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
pub(crate) fn resize_webview(
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
pub(crate) fn apply_windows_input_mode(
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

#[cfg(target_os = "windows")]
fn dispatch_webui_message(
    io: &Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
    pending: &Arc<Mutex<Vec<PendingCommand>>>,
    subscriptions: &Arc<Mutex<BTreeSet<String>>>,
    subscription_generation: &Arc<std::sync::atomic::AtomicU64>,
    source: UiInstanceId,
    body: &str,
) {
    let subscription = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("roundo_subscription").cloned());
    if let Some(subscription) = subscription {
        let version = subscription.get("version").and_then(Value::as_u64);
        let resource = subscription.get("resource").and_then(Value::as_str);
        let subscribed = subscription.get("subscribed").and_then(Value::as_bool);
        let Some((resource, subscribed)) = resource.zip(subscribed) else {
            log::warn!("Web UI Client Data subscription was rejected: invalid envelope");
            return;
        };
        if version != Some(1)
            || resource.is_empty()
            || resource.len() > 128
            || !resource
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            log::warn!(
                "Web UI Client Data subscription was rejected: invalid version or resource `{resource}`"
            );
            return;
        }
        let changed = {
            let mut subscriptions = subscriptions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if subscribed {
                subscriptions.insert(resource.to_owned())
            } else {
                subscriptions.remove(resource)
            }
        };
        if changed {
            subscription_generation.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            log::debug!(
                "Web UI instance {} {} Client Data resource `{resource}`",
                source.get(),
                if subscribed {
                    "subscribed to"
                } else {
                    "unsubscribed from"
                }
            );
        }
        return;
    }
    enqueue_webui_command(io, pending, Some(source), body);
}

pub(crate) fn enqueue_webui_command(
    io: &Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
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
    let Some(command) = transport.get("command").cloned() else {
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
        Some(io) => match io.submit(
            command,
            source.map_or(UiCommandSource::Host, UiCommandSource::WebView),
        ) {
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
pub(crate) fn resolve_webui_commands(
    mut executor: Option<bevy::ecs::system::NonSendMut<UiNavigationExecutor>>,
) {
    let Some(executor) = executor.as_deref_mut() else {
        return;
    };
    let mut resolved_any = false;
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
            resolved_any = true;
        }
    }
    if resolved_any {
        executor.webview_creation_cooldown = true;
    }
}

pub(crate) fn bridge_focus_request(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("roundo_focus_request").and_then(Value::as_bool))
        == Some(true)
}

pub(crate) fn bridge_handshake(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("roundo_bridge_ready").and_then(Value::as_bool))
        == Some(true)
}

pub(crate) fn response_script(request_id: u64, result: &Value) -> String {
    format!(
        "window.__roundoResolve({}, {});",
        request_id,
        serde_json::to_string(result).expect("JSON result is serializable")
    )
}

pub(crate) fn data_sync_script(resource: &str, data: &Value) -> String {
    format!(
        "window.__roundoSync({}, {});",
        serde_json::to_string(resource).expect("Client Data resource name serializes"),
        serde_json::to_string(data).expect("Client Data snapshot serializes")
    )
}

#[cfg(target_os = "windows")]
const WEBUI_BRIDGE_SCRIPT: &str = r#"(() => { let next = 1; let active = false; const pending = new Map(); const queued = []; const subscriptions = new Map(); const snapshots = new Map(); const post = body => active ? window.ipc.postMessage(body) : queued.push(body); window.__roundoActivate = () => { if (active) return; active = true; for (const body of queued.splice(0)) window.ipc.postMessage(body); }; window.__roundoResolve = (id, value) => { const resolve = pending.get(id); if (resolve) { pending.delete(id); resolve(value); } }; window.__roundoSync = (resource, data) => { snapshots.set(resource, data); const listeners = subscriptions.get(resource); if (!listeners) return; for (const listener of [...listeners]) { try { listener(data); } catch (error) { console.error('Roundo Client Data subscriber failed', resource, error); } } }; window.roundo = { execute(command) { return new Promise(resolve => { const id = next++; pending.set(id, resolve); post(JSON.stringify({ request_id: id, command })); }); }, subscribe(resource, listener) { if (typeof resource !== 'string' || typeof listener !== 'function') throw new TypeError('roundo.subscribe requires a resource name and listener'); let listeners = subscriptions.get(resource); if (!listeners) { listeners = new Set(); subscriptions.set(resource, listeners); } const first = listeners.size === 0; listeners.add(listener); if (first) post(JSON.stringify({roundo_subscription:{version:1,resource,subscribed:true}})); else if (snapshots.has(resource)) listener(snapshots.get(resource)); let live = true; return () => { if (!live) return; live = false; listeners.delete(listener); if (!listeners.size) { subscriptions.delete(resource); snapshots.delete(resource); post(JSON.stringify({roundo_subscription:{version:1,resource,subscribed:false}})); } }; } }; window.addEventListener('pointerdown',()=>{ if (active) window.ipc.postMessage(JSON.stringify({roundo_focus_request:true})); },true); window.ipc.postMessage(JSON.stringify({roundo_bridge_ready:true})); })();"#;

pub(crate) fn webui_initialization_script(definition: &str) -> String {
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
