//! Shared physical host-handler installation. No logical lifecycle ownership.

use super::*;
use webview2_com::{
    ContentLoadingEventHandler,
    Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_WEB_ERROR_STATUS, ICoreWebView2},
    NavigationCompletedEventHandler, NavigationStartingEventHandler, SourceChangedEventHandler,
};

/// Only explicitly installed host handlers; Wry's private registrations are not included.
/// This value has no Drop cleanup: storing or moving it performs no COM calls.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct NativeHandlerTokens {
    pub(super) navigation_starting: Option<i64>,
    pub(super) navigation_completed: Option<i64>,
    pub(super) frame_navigation_starting: Option<i64>,
    pub(super) source_changed: Option<i64>,
    pub(super) content_loading: Option<i64>,
}

impl NativeHandlerTokens {
    fn detach(
        &mut self,
        mut remove: impl FnMut(HostHandlerKind, i64) -> Result<(), String>,
    ) -> Result<(), String> {
        for (kind, slot) in [
            (
                HostHandlerKind::NavigationStarting,
                &mut self.navigation_starting,
            ),
            (
                HostHandlerKind::NavigationCompleted,
                &mut self.navigation_completed,
            ),
            (
                HostHandlerKind::FrameNavigationStarting,
                &mut self.frame_navigation_starting,
            ),
            (HostHandlerKind::SourceChanged, &mut self.source_changed),
            (HostHandlerKind::ContentLoading, &mut self.content_loading),
        ] {
            if let Some(token) = *slot {
                remove(kind, token)?;
                *slot = None;
            }
        }
        Ok(())
    }
    fn complete(&self) -> bool {
        self.navigation_starting.is_some()
            && self.navigation_completed.is_some()
            && self.frame_navigation_starting.is_some()
            && self.source_changed.is_some()
            && self.content_loading.is_some()
    }
}

#[derive(Clone, Copy, Debug)]
enum HostHandlerKind {
    NavigationStarting,
    NavigationCompleted,
    FrameNavigationStarting,
    SourceChanged,
    ContentLoading,
}

/// Always preserve the tokens, including when installation failed part way through.
#[must_use = "retain the successfully installed tokens with their physical WebView owner"]
#[derive(Debug)]
pub(super) struct HostHandlerInstall {
    pub(super) tokens: NativeHandlerTokens,
    pub(super) result: Result<(), String>,
}

impl HostHandlerInstall {
    fn new() -> Self {
        Self {
            tokens: NativeHandlerTokens::default(),
            result: Ok(()),
        }
    }

    /// True permits the next add; false freezes the successful prefix and first error.
    fn record(&mut self, kind: HostHandlerKind, result: Result<i64, String>) -> bool {
        if self.result.is_err() {
            return false;
        }
        match result {
            Ok(token) => {
                let slot = match kind {
                    HostHandlerKind::NavigationStarting => &mut self.tokens.navigation_starting,
                    HostHandlerKind::NavigationCompleted => &mut self.tokens.navigation_completed,
                    HostHandlerKind::FrameNavigationStarting => {
                        &mut self.tokens.frame_navigation_starting
                    }
                    HostHandlerKind::SourceChanged => &mut self.tokens.source_changed,
                    HostHandlerKind::ContentLoading => &mut self.tokens.content_loading,
                };
                *slot = Some(token);
                true
            }
            Err(error) => {
                self.result = Err(format!(
                    "cannot install WebView2 navigation status handlers: {error}"
                ));
                false
            }
        }
    }

    fn finish(mut self) -> Self {
        if self.result.is_ok() && !self.tokens.complete() {
            self.result = Err(
                "cannot install WebView2 navigation status handlers: incomplete installation"
                    .into(),
            );
        }
        self
    }
}

pub(super) struct HostNavigationHandlerInputs {
    pub(super) observation: NativeObservation,
    pub(super) callbacks: HostCallbackActivity,
    pub(super) view_token: NativeViewToken,
    pub(super) frame_definition: String,
    pub(super) wake: Arc<dyn Fn() + Send + Sync>,
    pub(super) subscriptions: Arc<Mutex<BTreeSet<String>>>,
    pub(super) subscription_generation: Arc<std::sync::atomic::AtomicU64>,
    pub(super) command_gate: Option<Arc<Mutex<StagedCommandGate>>>,
    pub(super) pending_commands: Option<Arc<Mutex<Vec<PendingCommand>>>>,
}

fn native_field_failed_and_wake(
    observation: &NativeObservation,
    wake: &(dyn Fn() + Send + Sync),
    token: NativeViewToken,
    reason: String,
) {
    observation.native_field_failed(token, reason);
    // native_field_failed has returned: never hold its mailbox lock across wake.
    wake();
}

/// Runs on the owning UI thread. Does not consume the Wry owner or drop partial tokens.
pub(super) fn install_host_navigation_handlers(
    core: &ICoreWebView2,
    inputs: HostNavigationHandlerInputs,
) -> HostHandlerInstall {
    let HostNavigationHandlerInputs {
        observation,
        callbacks,
        view_token,
        frame_definition: frame_navigation_definition,
        wake,
        subscriptions: content_subscriptions,
        subscription_generation: content_subscription_generation,
        command_gate,
        pending_commands,
    } = inputs;
    let starting_callbacks = callbacks.clone();
    let completed_callbacks = callbacks.clone();
    let content_callbacks = callbacks.clone();
    let source_callbacks = callbacks.clone();
    let frame_callbacks = callbacks;
    let starting_observation = observation.clone();
    let completed_observation = observation.clone();
    let content_observation = observation.clone();
    let source_observation = observation.clone();
    let frame_observation = observation;
    let starting_wake = wake.clone();
    let completed_wake = wake.clone();
    let content_wake = wake.clone();
    let source_wake = wake.clone();
    let frame_wake = wake;

    let create_navigation_handler = NavigationStartingEventHandler::create;
    let starting_handler = create_navigation_handler(Box::new(move |_sender, args| {
        let _guard = starting_callbacks.enter_for(view_token);
        let Some(args) = args else {
            native_field_failed_and_wake(
                &starting_observation,
                starting_wake.as_ref(),
                view_token,
                "WebView2 NavigationStarting args missing".into(),
            );
            return Ok(());
        };
        let mut raw_uri = windows::core::PWSTR::null();
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { args.Uri(&mut raw_uri) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &starting_observation,
                starting_wake.as_ref(),
                view_token,
                format!("WebView2 NavigationStarting.Uri failed: {error}"),
            );
        })?;
        let uri = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
        let mut navigation_id = 0;
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { args.NavigationId(&mut navigation_id) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &starting_observation,
                starting_wake.as_ref(),
                view_token,
                format!("WebView2 NavigationStarting.NavigationId failed: {error}"),
            );
        })?;
        let mut cancel = windows::core::BOOL::default();
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { args.Cancel(&mut cancel) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &starting_observation,
                starting_wake.as_ref(),
                view_token,
                format!("WebView2 NavigationStarting.Cancel failed: {error}"),
            );
        })?;
        log::debug!(
            "WebView2 native navigation starting: view_token={view_token:?}, id={navigation_id}, uri={uri:?}, cancel={}",
            cancel.as_bool()
        );
        starting_observation.navigation_starting(view_token, navigation_id, &uri, cancel.as_bool());
        starting_wake();
        Ok(())
    }));
    let completed_handler = NavigationCompletedEventHandler::create(Box::new(
        move |_sender, args| {
            let _guard = completed_callbacks.enter_for(view_token);
            let Some(args) = args else {
                native_field_failed_and_wake(
                    &completed_observation,
                    completed_wake.as_ref(),
                    view_token,
                    "WebView2 NavigationCompleted args missing".into(),
                );
                return Ok(());
            };
            let mut navigation_id = 0;
            // SAFETY: valid UI-thread COM callback interface; initialized out value
            // is read only after this getter succeeds (the error propagates).
            unsafe { args.NavigationId(&mut navigation_id) }.inspect_err(|error| {
                native_field_failed_and_wake(
                    &completed_observation,
                    completed_wake.as_ref(),
                    view_token,
                    format!("WebView2 NavigationCompleted.NavigationId failed: {error}"),
                );
            })?;
            log::debug!(
                "WebView2 native navigation completed: view_token={view_token:?}, id={navigation_id}"
            );
            let mut success = windows::core::BOOL::default();
            // SAFETY: valid UI-thread COM callback interface; initialized out value
            // is read only after this getter succeeds (the error propagates).
            unsafe { args.IsSuccess(&mut success) }.inspect_err(|error| {
                native_field_failed_and_wake(
                    &completed_observation,
                    completed_wake.as_ref(),
                    view_token,
                    format!("WebView2 NavigationCompleted.IsSuccess failed: {error}"),
                );
            })?;
            let result = if success.as_bool() {
                Ok(())
            } else {
                let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                // SAFETY: valid UI-thread COM callback interface; initialized out value
                // is read only after this getter succeeds (the error propagates).
                unsafe { args.WebErrorStatus(&mut status) }.inspect_err(|error| {
                    native_field_failed_and_wake(
                        &completed_observation,
                        completed_wake.as_ref(),
                        view_token,
                        format!("WebView2 NavigationCompleted.WebErrorStatus failed: {error}"),
                    );
                })?;
                Err(format!("WebView2 navigation failed with status {status:?}"))
            };
            completed_observation.completed(view_token, navigation_id, result);
            completed_wake();
            Ok(())
        },
    ));
    let content_handler = ContentLoadingEventHandler::create(Box::new(move |sender, args| {
        let _guard = content_callbacks.enter_for(view_token);
        let (Some(sender), Some(args)) = (sender, args) else {
            native_field_failed_and_wake(
                &content_observation,
                content_wake.as_ref(),
                view_token,
                "WebView2 ContentLoading sender or args missing".into(),
            );
            return Ok(());
        };
        let mut navigation_id = 0;
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { args.NavigationId(&mut navigation_id) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &content_observation,
                content_wake.as_ref(),
                view_token,
                format!("WebView2 ContentLoading.NavigationId failed: {error}"),
            );
        })?;
        let mut raw_uri = windows::core::PWSTR::null();
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { sender.Source(&mut raw_uri) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &content_observation,
                content_wake.as_ref(),
                view_token,
                format!("WebView2 ContentLoading.Source failed: {error}"),
            );
        })?;
        let source = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
        let installed = staging::document_transport_content_loading(
            &content_observation,
            view_token,
            navigation_id,
            &source,
            &content_subscriptions,
            &content_subscription_generation,
        );
        if installed {
            if let Some(gate) = &command_gate {
                gate.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .new_document();
            }
            if let Some(pending) = &pending_commands {
                pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
            }
        }
        log::debug!(
            "WebView2 native content loading: view_token={view_token:?}, id={navigation_id}, source={source:?}, installed={installed}"
        );
        content_wake();
        Ok(())
    }));
    let source_handler = SourceChangedEventHandler::create(Box::new(move |sender, args| {
        let _guard = source_callbacks.enter_for(view_token);
        let (Some(sender), Some(args)) = (sender, args) else {
            native_field_failed_and_wake(
                &source_observation,
                source_wake.as_ref(),
                view_token,
                "WebView2 SourceChanged sender or args missing".into(),
            );
            return Ok(());
        };
        let mut is_new_document = windows::core::BOOL::default();
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { args.IsNewDocument(&mut is_new_document) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &source_observation,
                source_wake.as_ref(),
                view_token,
                format!("WebView2 SourceChanged.IsNewDocument failed: {error}"),
            );
        })?;
        let mut raw_uri = windows::core::PWSTR::null();
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { sender.Source(&mut raw_uri) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &source_observation,
                source_wake.as_ref(),
                view_token,
                format!("WebView2 SourceChanged.Source failed: {error}"),
            );
        })?;
        let source = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
        log::debug!(
            "WebView2 source identity SourceChanged: view_token={view_token:?}, is_new_document={}, source={source:?}",
            is_new_document.as_bool()
        );
        source_observation.source_changed(view_token, is_new_document.as_bool(), &source);
        source_wake();
        Ok(())
    }));
    let frame_handler = create_navigation_handler(Box::new(move |_sender, args| {
        let _guard = frame_callbacks.enter_for(view_token);
        let Some(args) = args else {
            native_field_failed_and_wake(
                &frame_observation,
                frame_wake.as_ref(),
                view_token,
                "WebView2 FrameNavigationStarting args missing".into(),
            );
            return Ok(());
        };
        let mut raw_uri = windows::core::PWSTR::null();
        // SAFETY: valid UI-thread COM callback interface; initialized out value
        // is read only after this getter succeeds (the error propagates).
        unsafe { args.Uri(&mut raw_uri) }.inspect_err(|error| {
            native_field_failed_and_wake(
                &frame_observation,
                frame_wake.as_ref(),
                view_token,
                format!("WebView2 FrameNavigationStarting.Uri failed: {error}"),
            );
        })?;
        let uri = webview2_com::CoTaskMemPWSTR::from(raw_uri).to_string();
        let allowed = frame_navigation_allowed(&frame_navigation_definition, &uri);
        if !allowed {
            log::warn!("blocked cross-Definition or external frame navigation: `{uri}`");
            // SAFETY: callback runs on the owning UI thread with valid event args.
            unsafe { args.SetCancel(true) }.inspect_err(|error| {
                native_field_failed_and_wake(
                    &frame_observation,
                    frame_wake.as_ref(),
                    view_token,
                    format!("WebView2 FrameNavigationStarting.SetCancel failed: {error}"),
                );
            })?;
        }
        Ok(())
    }));

    let mut installation = HostHandlerInstall::new();
    let mut token = 0;
    // SAFETY: valid core on its owning UI thread; handler lives through add.
    // The out token is recorded only on Ok, including the valid token value 0.
    let result = unsafe { core.add_NavigationStarting(&starting_handler, &mut token) };
    if !installation.record(
        HostHandlerKind::NavigationStarting,
        result.map(|_| token).map_err(|e| e.to_string()),
    ) {
        return installation.finish();
    }
    let mut token = 0;
    // SAFETY: same UI-thread interface/handler lifetime; failed out token is not read.
    let result = unsafe { core.add_NavigationCompleted(&completed_handler, &mut token) };
    if !installation.record(
        HostHandlerKind::NavigationCompleted,
        result.map(|_| token).map_err(|e| e.to_string()),
    ) {
        return installation.finish();
    }
    let mut token = 0;
    // SAFETY: same UI-thread interface/handler lifetime; failed out token is not read.
    let result = unsafe { core.add_FrameNavigationStarting(&frame_handler, &mut token) };
    if !installation.record(
        HostHandlerKind::FrameNavigationStarting,
        result.map(|_| token).map_err(|e| e.to_string()),
    ) {
        return installation.finish();
    }
    let mut token = 0;
    // SAFETY: same UI-thread interface/handler lifetime; failed out token is not read.
    let result = unsafe { core.add_SourceChanged(&source_handler, &mut token) };
    if !installation.record(
        HostHandlerKind::SourceChanged,
        result.map(|_| token).map_err(|e| e.to_string()),
    ) {
        return installation.finish();
    }
    let mut token = 0;
    // SAFETY: same UI-thread interface/handler lifetime; failed out token is not read.
    let result = unsafe { core.add_ContentLoading(&content_handler, &mut token) };
    installation.record(
        HostHandlerKind::ContentLoading,
        result.map(|_| token).map_err(|e| e.to_string()),
    );
    installation.finish()
}

/// Detaches only host registrations. A failed remove retains its exact token
/// and the unprocessed suffix, so the physical owner cannot be silently dropped.
pub(super) fn remove_host_navigation_handlers(
    core: &ICoreWebView2,
    tokens: &mut NativeHandlerTokens,
) -> Result<(), String> {
    tokens.detach(|kind, token| {
        // SAFETY: caller retains the live core on the UI apartment; every token
        // came from a successful add on this core. It is cleared only on Ok.
        unsafe {
            match kind {
                HostHandlerKind::NavigationStarting => core.remove_NavigationStarting(token),
                HostHandlerKind::NavigationCompleted => core.remove_NavigationCompleted(token),
                HostHandlerKind::FrameNavigationStarting => {
                    core.remove_FrameNavigationStarting(token)
                }
                HostHandlerKind::SourceChanged => core.remove_SourceChanged(token),
                HostHandlerKind::ContentLoading => core.remove_ContentLoading(token),
            }
        }
        .map_err(|error| format!("cannot detach {kind:?}: {error}"))
    })
}
