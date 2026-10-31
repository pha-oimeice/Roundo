//! UI-thread Wry ownership. This adapter executes decisions; it never commits a UI.

use super::runtime::{NativeInput, ViewDriver, drive_views};
use super::*;
use crate::state::{
    NativeActivity, NativeFact, RecoveryActivity, RecoveryId, UiAction, UiDocumentProof,
    UiOpeningId, UiViewLifetime,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) struct NativeContext {
    pub(super) parent: Option<bevy::prelude::Entity>,
    pub(super) wake: Arc<dyn Fn() + Send + Sync>,
    pub(super) command_io: Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
    pub(super) host_script: Option<String>,
}
impl Default for NativeContext {
    fn default() -> Self {
        Self {
            parent: None,
            wake: Arc::new(|| {}),
            command_io: None,
            host_script: None,
        }
    }
}

/// One map owns every physical view, independent of logical phase. Tokens remain
/// attached after partial installation/Close failures; only successful Close and
/// Drop remove an owner. A failed Close is sticky and freezes later creation.
#[derive(Default)]
pub(super) struct WryDriver {
    pub(super) views: BTreeMap<UiOpeningId, NativeView>,
    pub(super) callbacks: HostCallbackActivity,
    pub(super) subscription_generation: Arc<AtomicU64>,
    pub(super) context: NativeContext,
    pub(super) recovery: Option<RecoveryView>,
    pub(super) recovery_close_failed: bool,
    pub(super) fatal: Vec<String>,
    pub(super) recovery_actions: Vec<RecoveryAction>,
}

pub(super) type DocumentMessageQueue = Arc<Mutex<Vec<(UiDocumentProof, Value)>>>;

pub(super) struct NativeView {
    pub(super) webview: Rc<wry::WebView>,
    pub(super) handlers: driver::NativeHandlerTokens,
    pub(super) pending: Arc<Mutex<Vec<PendingCommand>>>,
    pub(super) host_messages: DocumentMessageQueue,
    pub(super) command_io: Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
    pub(super) subscriptions: Arc<Mutex<BTreeSet<String>>>,
    pub(super) scripts: ScriptTracker,
    pub(super) gate: Arc<Mutex<StagedCommandGate>>,
    pub(super) observation: NativeObservation,
    pub(super) definition: String,
    pub(super) active: bool,
    pub(super) last_presentation: Option<crate::state::UiPresentation>,
}

/// Per-document transport state is revoked on native installation and permanently
/// retired on logical removal. Queued messages never survive either transition.
#[derive(Default)]
pub(crate) struct StagedCommandGate {
    admitted: bool,
    retired: bool,
    queued: Vec<String>,
}
impl StagedCommandGate {
    pub(crate) fn queue_until_commit(&mut self, body: &str) -> bool {
        if self.admitted && !self.retired {
            return false;
        }
        if !self.retired && self.queued.len() < crate::registry::MAX_PREPARED_COMMANDS {
            self.queued.push(body.to_owned());
        }
        true
    }
    pub(crate) fn release_queued(&mut self) -> Vec<String> {
        if self.retired {
            return Vec::new();
        }
        self.admitted = true;
        std::mem::take(&mut self.queued)
    }
    pub(crate) fn new_document(&mut self) {
        self.admitted = false;
        self.queued.clear();
    }
    fn retire(&mut self) {
        self.retired = true;
        self.admitted = false;
        self.queued.clear();
    }
}

impl WryDriver {
    pub(super) fn take_document_messages(
        &self,
        lifetime: &UiViewLifetime,
    ) -> Vec<(UiInstanceId, Value)> {
        let mut output = Vec::new();
        for (opening, view) in &self.views {
            let queued =
                std::mem::take(&mut *view.host_messages.lock().unwrap_or_else(|p| p.into_inner()));
            if !view.active || lifetime.committed_identity(*opening).is_none() {
                continue;
            }
            let current = view
                .observation
                .confirmed_document()
                .map(|(proof, _)| proof);
            output.extend(queued.into_iter().filter_map(|(proof, payload)| {
                (current == Some(proof)).then_some((opening.instance_id(), payload))
            }));
        }
        output
    }

    pub(super) fn publish_event(
        &self,
        lifetime: &UiViewLifetime,
        target: UiInstanceId,
        resource: &str,
        data: &Value,
    ) -> usize {
        let script = format!(
            "window.__roundoEvent({}, {});",
            serde_json::to_string(resource).expect("resource serializes"),
            data
        );
        for (opening, view) in &self.views {
            if opening.instance_id() != target
                || !view.active
                || lifetime.committed_identity(*opening).is_none()
                || view.observation.confirmed_document().is_none()
                || !view
                    .subscriptions
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains(resource)
            {
                continue;
            }
            return match evaluate_script_transaction(&view.webview, &view.scripts, &script) {
                Ok(()) => 1,
                Err(error) => {
                    log::error!("cannot deliver UI event `{resource}`: {error}");
                    0
                }
            };
        }
        0
    }

    pub(super) fn live_views<'a>(
        &'a self,
        lifetime: &'a UiViewLifetime,
    ) -> impl Iterator<Item = &'a NativeView> {
        self.views.iter().filter_map(move |(opening, view)| {
            (view.active
                && lifetime.committed_identity(*opening).is_some()
                && view.observation.confirmed_document().is_some())
            .then_some(view)
        })
    }
    pub(super) fn publish_data(
        &self,
        lifetime: &UiViewLifetime,
        resource: &str,
        data: &Value,
    ) -> usize {
        let script = data_sync_script(resource, data);
        let mut delivered = 0;
        for view in self.live_views(lifetime) {
            if view
                .subscriptions
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(resource)
            {
                match evaluate_script_transaction(&view.webview, &view.scripts, &script) {
                    Ok(()) => delivered += 1,
                    Err(error) => {
                        log::error!("cannot synchronize Client Data `{resource}`: {error}")
                    }
                }
            }
        }
        delivered
    }
}

impl ViewDriver for WryDriver {
    fn revoke_stale(&mut self, lifetime: &UiViewLifetime) {
        for (opening, view) in &mut self.views {
            if view.active && lifetime.committed_identity(*opening).is_none() {
                view.active = false;
                view.gate.lock().unwrap_or_else(|p| p.into_inner()).retire();
                // Dropping a response observer does not cancel already accepted
                // business work, but a dead document must not block creation.
                view.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clear();
                let mut subscriptions =
                    view.subscriptions.lock().unwrap_or_else(|p| p.into_inner());
                if !subscriptions.is_empty() {
                    subscriptions.clear();
                    self.subscription_generation.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
    }

    fn poll(&mut self, lifetime: &UiViewLifetime, manager: &UiLifecycleManager) -> NativeInput {
        let mut facts = Vec::new();
        let mut focus_requests = Vec::new();
        // Complete all potentially reentrant native work before draining ANY
        // view. A probe for a later view may synchronously update an earlier
        // view's mailbox; the decision must see those facts in this snapshot.
        self.poll_recovery(lifetime, &mut facts);
        for view in self.views.values() {
            probe_document_readiness(&view.webview, &view.scripts, &view.observation, {
                let wake = self.context.wake.clone();
                move || wake()
            });
        }
        for (opening, view) in &self.views {
            let drain = view.observation.drain();
            if drain.token != view.observation.token() {
                facts.push(NativeFact::NativeFault {
                    message: "foreign physical observation owner".into(),
                });
                continue;
            }
            if let Some(message) = drain.fault {
                facts.push(NativeFact::NativeFault { message });
            }
            let confirmed = drain.document_update.and(drain.confirmed_document);
            facts.extend(document_facts(
                *opening,
                &view.definition,
                drain.events,
                confirmed,
                manager,
            ));
            if drain.focus_requested {
                focus_requests.push(*opening);
            }
        }
        let activity = NativeActivity {
            parent_available: self.context.parent.is_some_and(|entity| {
                bevy_winit::WINIT_WINDOWS.with(|all| all.borrow().get_window(entity).is_some())
            }),
            callback_depth: Some(self.callbacks.depth()),
            scripts: self
                .views
                .iter()
                .map(|(id, v)| (*id, Some(v.scripts.pending())))
                .collect(),
            navigation_pending: self
                .views
                .iter()
                .map(|(id, v)| (*id, Some(v.observation.snapshot().navigation_pending)))
                .collect(),
            commands_pending: self.views.values().any(|view| {
                view.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .iter()
                    .any(|command| {
                        command
                            .call
                            .as_ref()
                            .is_none_or(|call| lifetime.command_blocks_creation(call))
                    })
            }),
            recovery: RecoveryActivity {
                exists: self.recovery.is_some(),
                scripts: self.recovery.as_ref().map(|v| v.scripts.pending()),
                navigation_pending: self
                    .recovery
                    .as_ref()
                    .map(|v| v.observation.snapshot().navigation_pending),
                create_requested: lifetime.recovery_request().is_some(),
                close_requested: self.recovery.is_some() && lifetime.recovery_request().is_none(),
                close_failed: self.recovery_close_failed,
            },
        };
        NativeInput {
            activity,
            facts,
            focus_requests,
        }
    }

    fn execute(&mut self, action: UiAction, manager: &UiLifecycleManager) -> Vec<NativeFact> {
        match action {
            UiAction::CreateBlank {
                opening,
                action_id,
                descriptor,
            } => self.create_view(opening, action_id, descriptor, manager),
            UiAction::LoadDocument {
                opening,
                action_id,
                target,
            } => {
                let result = self
                    .views
                    .get(&opening)
                    .ok_or_else(|| "missing physical view".to_owned())
                    .and_then(|v| {
                        v.webview
                            .load_url(&webview2_resource_url(target.resource(), &target.path))
                            .map_err(|e| e.to_string())
                    });
                vec![NativeFact::LoadResult {
                    opening,
                    action_id,
                    result,
                }]
            }
            UiAction::SuspendPrepared { opening } => {
                if let Some(view) = self.views.get(&opening) {
                    staging::suspend_prepared_webview(view);
                }
                Vec::new()
            }
            UiAction::ResumePrepared { opening } => {
                if let Some(view) = self.views.get(&opening) {
                    staging::resume_prepared_webview(view);
                }
                Vec::new()
            }
            UiAction::ActivateCommittedAndReleaseQueued {
                opening,
                instance,
                expected_document,
            } => {
                let Some(view) = self.views.get_mut(&opening) else {
                    return vec![NativeFact::NativeFault {
                        message: "committed view has no physical owner".into(),
                    }];
                };
                // This fresh proof check is immediately before the reentrant call,
                // and repeated afterwards before any queued message gets authority.
                if view
                    .observation
                    .confirmed_document()
                    .map(|(proof, _)| proof)
                    != Some(expected_document)
                {
                    return vec![NativeFact::ActivationSkipped {
                        opening,
                        expected_document,
                    }];
                }
                if let Err(error) = evaluate_script_transaction(
                    &view.webview,
                    &view.scripts,
                    "window.__roundoActivate();",
                ) {
                    return vec![NativeFact::NativeFault {
                        message: format!("cannot activate UI bridge: {error}"),
                    }];
                }
                if view
                    .observation
                    .confirmed_document()
                    .map(|(proof, _)| proof)
                    != Some(expected_document)
                {
                    return vec![NativeFact::ActivationSkipped {
                        opening,
                        expected_document,
                    }];
                }
                view.active = true;
                let queued = view
                    .gate
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .release_queued();
                for body in queued {
                    if !command_response_allowed(
                        &view.observation,
                        view.observation.token(),
                        expected_document,
                        &view.gate,
                    ) {
                        break;
                    }
                    let rejected = dispatch_webui_message(
                        &view.command_io,
                        &view.pending,
                        &view.subscriptions,
                        &self.subscription_generation,
                        &view.host_messages,
                        Some(expected_document),
                        instance,
                        &body,
                    );
                    if let Some(rejected) = rejected {
                        resolve_command_rejection(
                            &RefCell::new(Rc::downgrade(&view.webview)),
                            &view.observation,
                            view.observation.token(),
                            Some(expected_document),
                            &view.gate,
                            rejected,
                            |webview, script| {
                                if let Err(error) =
                                    evaluate_script_transaction(webview, &view.scripts, script)
                                {
                                    log::error!("cannot resolve Web UI command: {error}");
                                }
                            },
                        );
                    }
                }
                Vec::new()
            }
            UiAction::ApplyPresentation {
                opening,
                action_id,
                presentation,
                expected_document,
            } => {
                let result = self
                    .views
                    .get_mut(&opening)
                    .ok_or_else(|| "missing physical presentation".to_owned())
                    .and_then(|view| apply_presentation(view, presentation, expected_document));
                vec![NativeFact::PresentationResult {
                    opening,
                    action_id,
                    result,
                }]
            }
            UiAction::BeginDestroying { opening, action_id } => {
                let result = self
                    .views
                    .get(&opening)
                    .ok_or_else(|| "missing destroying view".to_owned())
                    .and_then(|view| {
                        dispatch_lifecycle_event(
                            &view.webview,
                            &view.scripts,
                            "roundo:destroying",
                            "destroying",
                        )
                        .map_err(|e| e.to_string())
                    });
                match result {
                    Ok(()) => vec![NativeFact::DestroyingDispatched { opening, action_id }],
                    Err(message) => vec![NativeFact::NativeFault { message }],
                }
            }
            UiAction::CloseAndDropView { opening, action_id } => {
                let result = self
                    .views
                    .get_mut(&opening)
                    .ok_or_else(|| "missing closing view".to_owned())
                    .and_then(|view| close_view(view, &self.callbacks));
                if result.is_ok() {
                    // No transport/mailbox lock or World borrow is held over Drop.
                    drop(self.views.remove(&opening));
                } else if let Err(message) = &result {
                    self.fatal.push(message.clone());
                }
                vec![NativeFact::CloseResult {
                    opening,
                    action_id,
                    result,
                }]
            }
            UiAction::WorldHandoffNeeded { handoff } => match present_world(&self.context) {
                Ok(()) => vec![NativeFact::WorldPresented { handoff }],
                Err(message) => vec![NativeFact::NativeFault { message }],
            },
            UiAction::RecoveryHandoffNeeded { recovery } => {
                log::trace!("waiting for physical Recovery presentation: {recovery:?}");
                Vec::new() // acknowledged only by an actually presented surface in poll
            }
            UiAction::ReportFatalNativeFailure { message } => {
                self.fatal.push(message);
                Vec::new()
            }
            UiAction::ReportFatalCreateFailure { opening, message } => {
                self.fatal.push(format!("{opening:?}: {message}"));
                Vec::new()
            }
            UiAction::SettleOpenResponse { .. } => {
                unreachable!("the common pump owns response settlement")
            }
        }
    }
    fn create_recovery(&mut self, recovery: RecoveryId) -> Vec<NativeFact> {
        match self.build_recovery(recovery) {
            Ok(()) => Vec::new(),
            Err(message) => {
                self.fatal.push(message.clone());
                vec![NativeFact::NativeFault { message }]
            }
        }
    }
    fn close_recovery(&mut self) -> Vec<NativeFact> {
        let Some(view) = self.recovery.as_mut() else {
            return Vec::new();
        };
        let result = close_native_owner(
            &view.webview,
            &mut view.handlers,
            &view.scripts,
            &self.callbacks,
        );
        match result {
            Ok(()) => {
                drop(self.recovery.take());
                Vec::new()
            }
            Err(message) => {
                self.recovery_close_failed = true;
                self.fatal.push(message.clone());
                vec![NativeFact::NativeFault { message }]
            }
        }
    }
}

/// Projects FIFO native events without borrowing a later snapshot's identity for
/// an earlier completion. Raw handshake hints never become transaction proof.
pub(super) fn document_facts(
    opening: UiOpeningId,
    definition: &str,
    events: Vec<NativeEvent>,
    confirmed: Option<(UiDocumentProof, String)>,
    manager: &UiLifecycleManager,
) -> Vec<NativeFact> {
    let target = |uri: &str| {
        same_definition_path(definition, uri)
            .and_then(|path| manager.resolve_resource_path(definition, Some(&path)).ok())
    };
    let mut facts = Vec::new();
    for event in events {
        match event {
            NativeEvent::Started(stamp) => facts.push(NativeFact::NavigationStarted {
                opening,
                target: target(&stamp.uri),
                navigation_id: stamp.id,
                start_revision: stamp.revision,
            }),
            NativeEvent::Completed(stamp, result) => facts.push(NativeFact::NavigationCompleted {
                opening,
                navigation_id: stamp.id,
                start_revision: stamp.revision,
                result,
            }),
            NativeEvent::ContentLoading { proof, source } => {
                facts.push(NativeFact::DocumentInstalled {
                    opening,
                    proof,
                    target: target(&source),
                })
            }
            NativeEvent::SourceChanged {
                proof: Some(proof),
                source,
                ..
            } => facts.push(NativeFact::SourceQualificationChanged {
                opening,
                proof,
                target: target(&source),
            }),
            NativeEvent::BlankFinished => facts.push(NativeFact::BlankFinished { opening }),
            NativeEvent::HandshakeHint(stamp)
            | NativeEvent::ProbeFinished(stamp)
            | NativeEvent::Focus(stamp) => {
                log::trace!("native UI hint, not authority proof: {stamp:?}");
            }
            NativeEvent::SourceChanged {
                is_new_document,
                proof: None,
                ..
            } => {
                log::trace!("unbound native source change: new_document={is_new_document}");
            }
        }
    }
    if let Some((proof, _)) = confirmed {
        facts.push(NativeFact::Handshake { opening, proof });
    }
    facts
}

/// Temporarily take ownership out of World: reentrant native callbacks only
/// touch their owned mailboxes/endpoints, never an outstanding World borrow.
pub(crate) fn drive_native_views(world: &mut bevy::prelude::World) {
    let parent = world.query_filtered::<bevy::prelude::Entity, bevy::ecs::query::With<bevy::window::PrimaryWindow>>().iter(world).next();
    let proxy = world
        .get_resource::<bevy_winit::EventLoopProxyWrapper>()
        .map(|p| std::ops::Deref::deref(p).clone());
    let Some(mut executor) = world.remove_non_send::<UiNavigationExecutor>() else {
        return;
    };
    executor.native.context = NativeContext {
        parent,
        wake: Arc::new(move || {
            if let Some(proxy) = &proxy {
                let _ = proxy.send_event(bevy_winit::WinitUserEvent::WakeUp);
            }
        }),
        command_io: world
            .get_non_send::<WebUiCommandEndpoint>()
            .and_then(|endpoint| endpoint.io.clone()),
        host_script: world
            .get_resource::<UiHostDocumentScript>()
            .map(|script| script.0.clone()),
    };
    // The manager is detached too; no native callback may acquire it implicitly.
    let mut manager = world
        .remove_resource::<UiLifecycleManager>()
        .expect("UI manager installed before native adapter");
    drive_views(
        &mut executor.lifetime,
        &mut manager,
        &mut executor.native,
        Instant::now(),
    );
    world.insert_resource(manager);
    for action in std::mem::take(&mut executor.native.recovery_actions) {
        world.write_message(RecoveryActionRequest(action));
    }
    for message in std::mem::take(&mut executor.native.fatal) {
        log::error!("native Web UI failed closed: {message}");
        world.write_message(bevy::app::AppExit::error());
    }
    world.insert_non_send(executor);
}

/// Pure host guards release their locks before the external, reentrant boundary.
fn command_response_allowed(
    observation: &NativeObservation,
    token: NativeViewToken,
    proof: UiDocumentProof,
    gate: &Arc<Mutex<StagedCommandGate>>,
) -> bool {
    let admitted = {
        let gate = gate.lock().unwrap_or_else(|p| p.into_inner());
        gate.admitted && !gate.retired
    };
    admitted
        && observation.token() == token
        && observation.confirmed_document().map(|(current, _)| current) == Some(proof)
}

/// The callback owns only a weak slot, never the WebView that owns the callback.
/// Neither slot nor transport/observation borrows survive the native call.
pub(super) fn resolve_command_rejection<W>(
    slot: &RefCell<Weak<W>>,
    observation: &NativeObservation,
    token: NativeViewToken,
    proof: Option<UiDocumentProof>,
    gate: &Arc<Mutex<StagedCommandGate>>,
    (request_id, result): (u64, Value),
    evaluate: impl FnOnce(&W, &str),
) {
    let Some(proof) = proof else { return };
    let webview = { slot.borrow().upgrade() };
    let Some(webview) = webview else { return };
    if !command_response_allowed(observation, token, proof, gate) {
        return;
    }
    evaluate(&webview, &response_script(request_id, &result));
}

pub(super) fn evaluate_script_transaction(
    webview: &wry::WebView,
    scripts: &ScriptTracker,
    script: &str,
) -> wry::Result<()> {
    let ticket = scripts.begin();
    let completed = ticket.clone();
    let callbacks = scripts.clone();
    let result = webview.evaluate_script_with_callback(script, move |_| {
        let _guard = callbacks.enter_callback();
        completed.complete();
    });
    if result.is_err() {
        ticket.complete();
    }
    result
}

fn probe_document_readiness(
    webview: &wry::WebView,
    scripts: &ScriptTracker,
    observation: &NativeObservation,
    wake: impl Fn() + Send + 'static,
) {
    let Some(probe) = observation.take_probe() else {
        return;
    };
    let ticket = scripts.begin();
    let completed = ticket.clone();
    let callbacks = scripts.clone();
    let observation = observation.clone();
    let result = webview.evaluate_script_with_callback(READINESS_PROBE, move |response| {
        let _guard = callbacks.enter_callback();
        completed.complete();
        observation.probe_completed(&probe, &response);
        wake();
    });
    if let Err(error) = result {
        ticket.complete();
        log::warn!("cannot probe UI readiness: {error}");
    }
}
