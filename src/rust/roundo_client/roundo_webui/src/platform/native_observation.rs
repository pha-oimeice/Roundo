//! Physical-view observations only: no World, lifecycle tree or transaction owner.
//! Snapshots are owned; Start, installed document and original target stay distinct.

use crate::state::UiDocumentProof;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

const MAX_NATIVE_EVENTS: usize = 256;
static NEXT_VIEW_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NativeViewToken(u64);

impl NativeViewToken {
    pub(super) fn allocate() -> Self {
        Self(
            NEXT_VIEW_TOKEN
                .try_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                    next.checked_add(1)
                })
                .expect("physical WebView identity space exhausted"),
        )
    }
}

/// Tracks only host-registered native callbacks, not WebView2 internal COM work.
#[derive(Clone, Default)]
pub(super) struct HostCallbackActivity(Arc<AtomicUsize>);

impl HostCallbackActivity {
    pub(super) fn enter(&self) -> HostCallbackGuard {
        self.0.fetch_add(1, Ordering::AcqRel);
        HostCallbackGuard {
            activity: self.clone(),
            _token: None,
        }
    }

    pub(super) fn enter_for(&self, token: NativeViewToken) -> HostCallbackGuard {
        self.0.fetch_add(1, Ordering::AcqRel);
        HostCallbackGuard {
            activity: self.clone(),
            _token: Some(token),
        }
    }

    pub(super) fn depth(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

pub(super) struct HostCallbackGuard {
    activity: HostCallbackActivity,
    _token: Option<NativeViewToken>,
}

impl Drop for HostCallbackGuard {
    fn drop(&mut self) {
        self.activity.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A ticket must be explicitly completed on callback or synchronous native Err.
/// Dropping it is not evidence that native work finished.
#[derive(Clone, Default)]
pub(super) struct ScriptTracker {
    count: Arc<AtomicUsize>,
    callbacks: HostCallbackActivity,
    token: Option<NativeViewToken>,
}

impl ScriptTracker {
    pub(super) fn new(callbacks: HostCallbackActivity, token: NativeViewToken) -> Self {
        Self {
            count: Arc::default(),
            callbacks,
            token: Some(token),
        }
    }

    pub(super) fn enter_callback(&self) -> HostCallbackGuard {
        match self.token {
            Some(token) => self.callbacks.enter_for(token),
            None => self.callbacks.enter(),
        }
    }

    pub(super) fn begin(&self) -> ScriptCompletionTicket {
        self.count.fetch_add(1, Ordering::AcqRel);
        ScriptCompletionTicket(Arc::new(ScriptCompletion {
            tracker: self.clone(),
            completed: AtomicBool::new(false),
        }))
    }

    pub(super) fn pending(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

struct ScriptCompletion {
    tracker: ScriptTracker,
    completed: AtomicBool,
}

#[derive(Clone)]
pub(super) struct ScriptCompletionTicket(Arc<ScriptCompletion>);

impl ScriptCompletionTicket {
    pub(super) fn complete(&self) {
        if !self.0.completed.swap(true, Ordering::AcqRel) {
            self.0.tracker.count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NavigationStamp {
    pub(super) token: NativeViewToken,
    pub(super) id: u64,
    pub(super) uri: String,
    pub(super) revision: u64,
}

#[derive(Clone, Debug)]
pub(super) struct ProbeTicket {
    start: NavigationStamp,
    document: u64,
    source_revision: u64,
    uri: String,
}

#[derive(Clone, Debug)]
pub(super) enum NativeEvent {
    Started(NavigationStamp),
    Completed(NavigationStamp, Result<(), String>),
    ContentLoading {
        proof: UiDocumentProof,
        source: String,
    },
    BlankFinished,
    HandshakeHint(NavigationStamp),
    ProbeFinished(NavigationStamp),
    Focus(NavigationStamp),
    SourceChanged {
        is_new_document: bool,
        source: String,
        proof: Option<UiDocumentProof>,
    },
}

pub(super) struct ObservationSnapshot {
    pub(super) navigation_result: Option<Result<(), String>>,
    pub(super) navigation_pending: bool,
}

pub(super) struct ObservationDrain {
    pub(super) token: NativeViewToken,
    pub(super) confirmed_document: Option<(UiDocumentProof, String)>,
    pub(super) events: Vec<NativeEvent>,
    pub(super) document_update: Option<String>,
    pub(super) focus_requested: bool,
    pub(super) fault: Option<String>,
}

/// Native installation facts, retained while a later admitted Start is pending.
/// They cannot qualify that later Start for logical readiness.
struct DocumentFacts {
    start: NavigationStamp,
    identity: u64,
    source: String,
    source_revision: u64,
    native_success: bool,
    hint: bool,
    probe_dispatched: bool,
    confirmed: bool,
}

impl DocumentFacts {
    fn proof(&self) -> UiDocumentProof {
        UiDocumentProof {
            navigation_id: self.start.id,
            start_revision: self.start.revision,
            document_identity: self.identity,
            source_revision: self.source_revision,
        }
    }
}

struct ObservedFacts {
    stamp: Option<NavigationStamp>,
    revision: u64,
    navigation_result: Option<Result<(), String>>,
    load_error: Option<String>,
    initial_blank_finished: bool,
    /// Hint for the latest admitted Start, before its document is installed.
    hint: bool,
    document: Option<DocumentFacts>,
    /// Authority is the accepted ContentLoading, never SourceChanged alone.
    installed_revision: Option<u64>,
    source_revision: u64,
    document_update: bool,
    focus_requested: bool,
    events: VecDeque<NativeEvent>,
    faulted: bool,
    fault_reported: bool,
    fault_reason: Option<String>,
}

impl ObservedFacts {
    fn fail(&mut self, reason: Option<String>) {
        if self.faulted {
            return;
        }
        self.faulted = true;
        self.fault_reason = reason;
        self.hint = false;
        if let Some(document) = self.document.as_mut() {
            document.confirmed = false;
            document.hint = false;
        }
        self.document_update = false;
        self.focus_requested = false;
    }

    fn record(&mut self, event: NativeEvent) {
        if self.faulted {
            return;
        }
        if self.events.len() == MAX_NATIVE_EVENTS {
            self.fail(None);
        } else {
            self.events.push_back(event);
        }
    }
}

#[derive(Clone)]
pub(super) struct NativeObservation {
    token: NativeViewToken,
    definition: Arc<str>,
    facts: Arc<Mutex<ObservedFacts>>,
}

/// Match Wry's custom-protocol projection, retaining path/query, not fragment.
fn document_uri(uri: &str) -> String {
    let uri = uri.split('#').next().unwrap_or_default();
    match uri.strip_prefix("roundo-ui://resource/") {
        Some(path) => format!("http://roundo-ui.resource/{path}"),
        None => uri.to_owned(),
    }
}

impl NativeObservation {
    pub(super) fn new(definition: &str, _target_path: &str) -> Self {
        Self {
            token: NativeViewToken::allocate(),
            definition: definition.into(),
            facts: Arc::new(Mutex::new(ObservedFacts {
                stamp: None,
                revision: 0,
                navigation_result: None,
                load_error: None,
                initial_blank_finished: false,
                hint: false,
                document: None,
                installed_revision: None,
                source_revision: 0,
                document_update: false,
                focus_requested: false,
                events: VecDeque::new(),
                faulted: false,
                fault_reported: false,
                fault_reason: None,
            })),
        }
    }

    pub(super) fn token(&self) -> NativeViewToken {
        self.token
    }

    pub(super) fn navigation_starting(
        &self,
        token: NativeViewToken,
        navigation_id: u64,
        uri: &str,
        cancelled: bool,
    ) {
        if !cancelled {
            self.started(token, navigation_id, uri);
        }
    }

    /// Installs at most once for the current admitted Start revision, including
    /// same-URL reloads for which WebView2 does not emit SourceChanged(true).
    pub(super) fn content_loading(
        &self,
        token: NativeViewToken,
        navigation_id: u64,
        native_source: &str,
    ) -> bool {
        if token != self.token {
            return false;
        }
        let source = document_uri(native_source);
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if facts.faulted {
            return false;
        }
        let Some(start) = facts.stamp.clone().filter(|stamp| {
            stamp.token == token && stamp.id == navigation_id && stamp.uri == source
        }) else {
            return false;
        };
        if facts.installed_revision == Some(start.revision) {
            return false;
        }
        let hint = std::mem::take(&mut facts.hint);
        facts.document_update = false;
        facts.focus_requested = false;
        facts.source_revision += 1;
        facts.installed_revision = Some(start.revision);
        facts.document = Some(DocumentFacts {
            start: start.clone(),
            identity: facts.source_revision,
            source,
            source_revision: facts.source_revision,
            native_success: facts.navigation_result == Some(Ok(())),
            hint,
            probe_dispatched: false,
            confirmed: false,
        });
        let installed = facts.document.as_ref().expect("just installed");
        let event = NativeEvent::ContentLoading {
            proof: installed.proof(),
            source: installed.source.clone(),
        };
        facts.record(event);
        !facts.faulted
    }

    pub(super) fn source_changed(
        &self,
        token: NativeViewToken,
        is_new_document: bool,
        native_source: &str,
    ) {
        if token != self.token {
            return;
        }
        let source = document_uri(native_source);
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if facts.faulted {
            return;
        }
        let current_installed = facts
            .stamp
            .as_ref()
            .is_some_and(|stamp| facts.installed_revision == Some(stamp.revision));
        if is_new_document && !current_installed {
            // A true notification revokes the previous physical identity but
            // cannot install this Start. Retain only this Start's raw hint.
            facts.document = None;
            facts.document_update = false;
            facts.focus_requested = false;
        } else {
            let Some(document) = facts.document.as_ref() else {
                facts.record(NativeEvent::SourceChanged {
                    is_new_document,
                    source,
                    proof: None,
                });
                return;
            };
            if document.source == source {
                if is_new_document {
                    let proof = Some(document.proof());
                    facts.record(NativeEvent::SourceChanged {
                        is_new_document,
                        source,
                        proof,
                    });
                }
                return;
            }
            facts.source_revision += 1;
            let revision = facts.source_revision;
            let document = facts.document.as_mut().expect("bound document");
            document.source = source.clone();
            document.source_revision = revision;
            // A failed/pending old-source probe stays physically pending, but
            // this new source revision is allowed its own single probe.
            document.probe_dispatched = false;
            facts.document_update = document.confirmed;
            facts.focus_requested = false;
        }
        let proof = facts.document.as_ref().map(DocumentFacts::proof);
        facts.record(NativeEvent::SourceChanged {
            is_new_document,
            source,
            proof,
        });
    }

    /// Missing native arguments/getter errors are sticky, never default success.
    pub(super) fn native_field_failed(&self, token: NativeViewToken, reason: String) {
        if token == self.token {
            log::error!("WebView {token:?} native source identity failed: {reason}");
            self.facts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .fail(Some(reason));
        }
    }

    pub(super) fn started(&self, token: NativeViewToken, id: u64, uri: &str) {
        if token != self.token {
            return;
        }
        let uri = document_uri(uri);
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if facts.faulted
            || facts
                .stamp
                .as_ref()
                .is_some_and(|s| s.id == id && s.uri == uri)
        {
            return;
        }
        facts.revision += 1;
        let stamp = NavigationStamp {
            token,
            id,
            uri,
            revision: facts.revision,
        };
        facts.stamp = Some(stamp.clone());
        facts.navigation_result = None;
        facts.load_error = None;
        facts.hint = false;
        facts.document_update = false;
        facts.focus_requested = false;
        facts.record(NativeEvent::Started(stamp));
    }

    pub(super) fn completed(&self, token: NativeViewToken, id: u64, result: Result<(), String>) {
        if token != self.token {
            return;
        }
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        let Some(stamp) = facts.stamp.clone().filter(|s| s.id == id) else {
            return;
        };
        if facts.faulted || facts.navigation_result.as_ref() == Some(&result) {
            return;
        }
        if let Some(document) = facts.document.as_mut().filter(|d| d.start == stamp) {
            document.native_success = result.is_ok();
            if result.is_err() {
                document.confirmed = false;
            }
        }
        facts.navigation_result = Some(result.clone());
        facts.record(NativeEvent::Completed(stamp, result));
    }

    pub(super) fn page_finished(&self, token: NativeViewToken, uri: &str) {
        if token != self.token || uri != "about:blank" {
            return;
        }
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if !facts.initial_blank_finished && !facts.faulted {
            facts.initial_blank_finished = true;
            facts.record(NativeEvent::BlankFinished);
        }
    }

    fn matching_source(&self, facts: &ObservedFacts, token: NativeViewToken, source: &str) -> bool {
        !facts.faulted
            && token == self.token
            && super::same_definition_path(&self.definition, source).is_some()
            && facts
                .document
                .as_ref()
                .is_some_and(|document| document.source == document_uri(source))
    }

    pub(super) fn accepts_source(&self, token: NativeViewToken, source: &str) -> bool {
        let facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        self.matching_source(&facts, token, source)
    }

    pub(super) fn handshake_hint(&self, token: NativeViewToken, source: &str) {
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if facts.faulted
            || token != self.token
            || super::same_definition_path(&self.definition, source).is_none()
        {
            return;
        }
        let Some(stamp) = facts.stamp.clone() else {
            return;
        };
        let source = document_uri(source);
        if let Some(document) = facts.document.as_mut().filter(|d| d.start == stamp) {
            if document.source != source || document.hint {
                return;
            }
            document.hint = true;
        } else {
            if stamp.uri != source || facts.hint {
                return;
            }
            facts.hint = true;
        }
        facts.record(NativeEvent::HandshakeHint(stamp));
    }

    pub(super) fn focus_intent(&self, token: NativeViewToken, source: &str) {
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if !self.matching_source(&facts, token, source) {
            return;
        }
        facts.focus_requested = true;
        let stamp = facts
            .document
            .as_ref()
            .expect("source matched document")
            .start
            .clone();
        facts.record(NativeEvent::Focus(stamp));
    }

    /// Returns an owned ticket, releasing the mailbox lock before any Wry call.
    pub(super) fn take_probe(&self) -> Option<ProbeTicket> {
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if facts.faulted || facts.navigation_result != Some(Ok(())) || facts.load_error.is_some() {
            return None;
        }
        let stamp = facts.stamp.clone()?;
        let document = facts.document.as_mut().filter(|d| d.start == stamp)?;
        if !document.native_success
            || !document.hint
            || document.probe_dispatched
            || document.confirmed
            || super::same_definition_path(&self.definition, &document.source).is_none()
        {
            return None;
        }
        document.probe_dispatched = true;
        Some(ProbeTicket {
            start: stamp,
            document: document.identity,
            source_revision: document.source_revision,
            uri: document.source.clone(),
        })
    }

    pub(super) fn probe_completed(&self, ticket: &ProbeTicket, response: &str) {
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        if facts.faulted
            || facts.stamp.as_ref() != Some(&ticket.start)
            || facts.navigation_result != Some(Ok(()))
            || facts.load_error.is_some()
        {
            return;
        }
        let Some(document) = facts.document.as_mut().filter(|d| {
            d.start == ticket.start
                && d.identity == ticket.document
                && d.source_revision == ticket.source_revision
                && d.source == ticket.uri
                && d.probe_dispatched
                && d.native_success
        }) else {
            return;
        };
        let value = serde_json::from_str::<serde_json::Value>(response).ok();
        let ready = value.as_ref().is_some_and(|v| {
            v.get("href")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|uri| document_uri(uri) == ticket.uri)
                && v.get("sent").and_then(serde_json::Value::as_bool) == Some(true)
                && v.get("bridge").and_then(serde_json::Value::as_bool) == Some(true)
        });
        if ready && !document.confirmed {
            document.confirmed = true;
            facts.document_update = true;
        }
        facts.record(NativeEvent::ProbeFinished(ticket.start.clone()));
    }

    /// Fresh physical proof for guards immediately adjacent to native work.
    /// Unlike drain(), this query neither consumes events nor grants authority.
    pub(super) fn confirmed_document(&self) -> Option<(UiDocumentProof, String)> {
        let facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        let document = facts.document.as_ref()?;
        (!facts.faulted
            && facts.load_error.is_none()
            && facts.navigation_result == Some(Ok(()))
            && facts.stamp.as_ref() == Some(&document.start)
            && document.native_success
            && document.confirmed
            && super::same_definition_path(&self.definition, &document.source).is_some())
        .then(|| (document.proof(), document.source.clone()))
    }

    /// Native success/installation only; this is not Mod bridge readiness.
    /// The host Recovery Surface adds its own DOM proof before becoming visible.
    pub(super) fn loaded_document(&self) -> Option<UiDocumentProof> {
        let facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        let document = facts.document.as_ref()?;
        (!facts.faulted
            && facts.load_error.is_none()
            && facts.navigation_result == Some(Ok(()))
            && facts.stamp.as_ref() == Some(&document.start)
            && document.native_success)
            .then(|| document.proof())
    }

    pub(super) fn snapshot(&self) -> ObservationSnapshot {
        let facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        ObservationSnapshot {
            navigation_result: facts
                .load_error
                .as_ref()
                .map(|e| Err(e.clone()))
                .or_else(|| facts.navigation_result.clone()),
            navigation_pending: facts.stamp.is_some() && facts.navigation_result.is_none(),
        }
    }

    /// FIFO facts are owned, and stale document/focus intents never escape.
    /// Fault reporting is once-only; fault suppression of readiness is sticky.
    pub(super) fn drain(&self) -> ObservationDrain {
        let mut facts = self.facts.lock().unwrap_or_else(|p| p.into_inner());
        let document_update = std::mem::take(&mut facts.document_update);
        let focus = std::mem::take(&mut facts.focus_requested);
        let confirmed = !facts.faulted
            && facts.load_error.is_none()
            && facts
                .document
                .as_ref()
                .is_some_and(|d| d.confirmed && d.native_success);
        // Focus may still belong to the real original page after a canceled
        // redirect. Activation must never reuse that page's proof for a Start.
        let current_confirmed = confirmed
            && facts.navigation_result == Some(Ok(()))
            && facts.document.as_ref().is_some_and(|d| {
                facts.stamp.as_ref() == Some(&d.start)
                    && facts.installed_revision == Some(d.start.revision)
            });
        let fault = if facts.faulted && !facts.fault_reported {
            facts.fault_reported = true;
            Some(facts.fault_reason.clone().unwrap_or_else(|| format!(
                "WebView {:?} exceeded its {MAX_NATIVE_EVENTS}-event native observation mailbox",
                self.token
            )))
        } else {
            None
        };
        ObservationDrain {
            token: self.token,
            confirmed_document: (current_confirmed
                && facts.document.as_ref().is_some_and(|document| {
                    super::same_definition_path(&self.definition, &document.source).is_some()
                }))
            .then(|| {
                let document = facts.document.as_ref().expect("current confirmed document");
                (document.proof(), document.source.clone())
            }),
            events: facts.events.drain(..).collect(),
            document_update: (current_confirmed && document_update).then(|| {
                facts
                    .document
                    .as_ref()
                    .expect("confirmed document")
                    .source
                    .clone()
            }),
            focus_requested: confirmed && focus,
            fault,
        }
    }
}

pub(super) const READINESS_PROBE: &str = "({href:location.href,sent:window.__roundoBridgeHandshakeSent === true,bridge:typeof window.__roundoActivate === 'function' && typeof window.__roundoResolve === 'function' && typeof window.__roundoSync === 'function' && typeof window.roundo?.execute === 'function'})";
