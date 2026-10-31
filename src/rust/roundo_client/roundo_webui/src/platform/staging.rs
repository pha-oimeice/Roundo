//! Physical blank creation, handler installation and optional native suspension.

use super::*;
use crate::state::{NativeFact, UiActionId, UiOpeningId};
use driver::{HostHandlerInstall, HostNavigationHandlerInputs, install_host_navigation_handlers};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
use wry::WebViewBuilderExtWindows;

/// Records a genuine installation before resetting subscriptions. No observation
/// lock is held while touching transport state or calling native code.
pub(super) fn document_transport_content_loading(
    observation: &NativeObservation,
    token: NativeViewToken,
    navigation_id: u64,
    native_source: &str,
    subscriptions: &Arc<Mutex<BTreeSet<String>>>,
    subscription_generation: &Arc<std::sync::atomic::AtomicU64>,
) -> bool {
    if !observation.content_loading(token, navigation_id, native_source) {
        return false;
    }
    let mut subscriptions = subscriptions.lock().unwrap_or_else(|p| p.into_inner());
    if !subscriptions.is_empty() {
        subscriptions.clear();
        subscription_generation.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    true
}

impl WryDriver {
    pub(super) fn create_view(
        &mut self,
        opening: UiOpeningId,
        action_id: UiActionId,
        descriptor: PendingUiDescriptor,
        manager: &UiLifecycleManager,
    ) -> Vec<NativeFact> {
        let Some(definition) = manager.registry.resource(descriptor.target.resource()) else {
            return vec![NativeFact::CreateResult {
                opening,
                action_id,
                created_at: None,
                result: Err("pending definition disappeared".into()),
            }];
        };
        let registry = Arc::new(manager.registry.clone());
        let source_definition = definition.name.clone();
        let observation = NativeObservation::new(&source_definition, &descriptor.target.path);
        let token = observation.token();
        let scripts = ScriptTracker::new(self.callbacks.clone(), token);
        let pending = Arc::new(Mutex::new(Vec::new()));
        let host_messages: DocumentMessageQueue = Arc::new(Mutex::new(Vec::new()));
        let ipc_host_messages = host_messages.clone();
        let subscriptions = Arc::new(Mutex::new(BTreeSet::new()));
        let gate = Arc::new(Mutex::new(StagedCommandGate::default()));
        let mut script = webui_initialization_script(&source_definition);
        if let Some(host_script) = &self.context.host_script {
            script.push_str("\nif (location.href !== 'about:blank') {\n");
            script.push_str(host_script);
            script.push_str("\n}\n");
        }
        let initial_blank = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let navigation_definition = source_definition.clone();
        let protocol_definition = source_definition.clone();
        let callbacks = self.callbacks.clone();
        let navigation_callbacks = callbacks.clone();
        let protocol_callbacks = callbacks.clone();
        let page_callbacks = callbacks.clone();
        let ipc_callbacks = callbacks.clone();
        let page_observation = observation.clone();
        let ipc_observation = observation.clone();
        let ipc_pending = pending.clone();
        let ipc_subscriptions = subscriptions.clone();
        let ipc_gate = gate.clone();
        let webview_slot = Rc::new(RefCell::new(Weak::<wry::WebView>::new()));
        let ipc_webview = webview_slot.clone();
        let ipc_scripts = scripts.clone();
        let io = self.context.command_io.clone();
        let generation = self.subscription_generation.clone();
        let wake_page = self.context.wake.clone();
        let wake_ipc = self.context.wake.clone();
        let previous_focus = if descriptor.prefetched {
            // SAFETY: this function runs on the Bevy UI thread; returned HWND
            // is used only to restore the immediately preceding native focus.
            Some(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() })
        } else {
            None
        };
        let result = bevy_winit::WINIT_WINDOWS.with(|all| {
            let all = all.borrow();
            let window = self
                .context
                .parent
                .and_then(|entity| all.get_window(entity))
                .ok_or_else(|| "primary native window is not ready".to_owned())?;
            let size = window.inner_size();
            let scale = window.scale_factor();
            // Embedded application UI: keep DOM input, not browser chrome/actions.
            wry::WebViewBuilder::new()
                .with_devtools(false)
                .with_default_context_menus(false)
                .with_browser_accelerator_keys(false)
                .with_hotkeys_zoom(false)
                .with_bounds(webview_rect(UiBounds {
                    x: 0,
                    y: 0,
                    width: (size.width as f64 / scale).round() as u32,
                    height: (size.height as f64 / scale).round() as u32,
                }))
                .with_visible(false)
                .with_transparent(definition.world_visibility == UiWorldVisibility::Visible)
                .with_background_color(definition.background_color.into())
                .with_url("about:blank")
                .with_navigation_handler(move |url| {
                    let _guard = navigation_callbacks.enter_for(token);
                    (url == "about:blank"
                        && initial_blank.swap(false, std::sync::atomic::Ordering::AcqRel))
                        || top_level_navigation_allowed(&navigation_definition, &url)
                })
                .with_on_page_load_handler(move |event, url| {
                    let _guard = page_callbacks.enter_for(token);
                    if matches!(event, wry::PageLoadEvent::Finished) {
                        page_observation.page_finished(token, &url);
                        wake_page();
                    }
                })
                .with_custom_protocol("roundo-ui".into(), move |_, request| {
                    let _guard = protocol_callbacks.enter_for(token);
                    protocol_response(&registry, Some(&protocol_definition), request.uri())
                })
                .with_initialization_script(script)
                .with_ipc_handler(move |message| {
                    let _guard = ipc_callbacks.enter_for(token);
                    let source = message.uri().to_string();
                    if bridge_handshake(message.body()) {
                        ipc_observation.handshake_hint(token, &source);
                        wake_ipc();
                        return;
                    }
                    if !ipc_observation.accepts_source(token, &source) {
                        return;
                    }
                    if bridge_focus_request(message.body()) {
                        ipc_observation.focus_intent(token, &source);
                        wake_ipc();
                        return;
                    }
                    let queued = ipc_gate
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .queue_until_commit(message.body());
                    if !queued {
                        let proof = ipc_observation.confirmed_document().map(|(proof, _)| proof);
                        let rejected = dispatch_webui_message(
                            &io,
                            &ipc_pending,
                            &ipc_subscriptions,
                            &generation,
                            &ipc_host_messages,
                            proof,
                            opening.instance_id(),
                            message.body(),
                        );
                        if let Some(rejected) = rejected {
                            resolve_command_rejection(
                                &ipc_webview,
                                &ipc_observation,
                                token,
                                proof,
                                &ipc_gate,
                                rejected,
                                |webview, script| {
                                    if let Err(error) =
                                        evaluate_script_transaction(webview, &ipc_scripts, script)
                                    {
                                        log::error!("cannot resolve Web UI command: {error}");
                                    }
                                },
                            );
                        }
                    }
                    wake_ipc();
                })
                .build_as_child(&**window)
                .map_err(|e| e.to_string())
        });
        let created_at = result.as_ref().ok().map(|_| Instant::now());
        if let Some(previous) = previous_focus.filter(|h| !h.is_invalid()) {
            // SAFETY: same UI thread; restore the live focus HWND sampled just
            // before speculative creation. Failure is surfaced, not assumed.
            if let Err(error) =
                unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(previous)) }
            {
                log::warn!("cannot restore focus after blank prefetch: {error}");
            }
        }
        let webview = match result {
            Ok(view) => Rc::new(view),
            Err(message) => {
                return vec![NativeFact::CreateResult {
                    opening,
                    action_id,
                    created_at: None,
                    result: Err(message),
                }];
            }
        };
        *webview_slot.borrow_mut() = Rc::downgrade(&webview);
        let created_at = created_at.expect("successful builder returned a physical owner");
        use wry::WebViewExtWindows;
        let HostHandlerInstall { tokens, result } = install_host_navigation_handlers(
            &webview.webview(),
            HostNavigationHandlerInputs {
                observation: observation.clone(),
                callbacks,
                view_token: token,
                frame_definition: source_definition.clone(),
                wake: self.context.wake.clone(),
                subscriptions: subscriptions.clone(),
                subscription_generation: self.subscription_generation.clone(),
                command_gate: Some(gate.clone()),
                pending_commands: Some(pending.clone()),
            },
        );
        // Even a failed partial install keeps the actual Wry owner and prefix
        // tokens until the core requests a safe, explicitly acknowledged Close.
        self.views.insert(
            opening,
            NativeView {
                webview,
                handlers: tokens,
                pending,
                host_messages,
                command_io: self.context.command_io.clone(),
                subscriptions,
                scripts,
                gate,
                observation,
                definition: source_definition,
                active: false,
                last_presentation: None,
            },
        );
        vec![NativeFact::CreateResult {
            opening,
            action_id,
            created_at: Some(created_at),
            result,
        }]
    }
}

pub(super) fn suspend_prepared_webview(view: &NativeView) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::ICoreWebView2_3, TrySuspendCompletedHandler,
    };
    use windows::core::Interface;
    use wry::WebViewExtWindows;
    let Ok(core): Result<ICoreWebView2_3, _> = view.webview.webview().cast() else {
        return;
    };
    let ticket = view.scripts.begin();
    let completed = ticket.clone();
    let callbacks = view.scripts.clone();
    let handler = TrySuspendCompletedHandler::create(Box::new(move |_, _| {
        let _guard = callbacks.enter_callback();
        completed.complete();
        Ok(())
    }));
    // SAFETY: live interface in its UI apartment; the handler owns its ticket
    // until the actual completion. Synchronous failure settles that ticket.
    if let Err(error) = unsafe { core.TrySuspend(&handler) } {
        ticket.complete();
        log::debug!("optional UI suspension failed: {error}");
    }
}
pub(super) fn resume_prepared_webview(view: &NativeView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use windows::core::Interface;
    use wry::WebViewExtWindows;
    let Ok(core): Result<ICoreWebView2_3, _> = view.webview.webview().cast() else {
        return;
    };
    // SAFETY: live interface on its owning UI thread; no callback borrow held.
    if let Err(error) = unsafe { core.Resume() } {
        log::debug!("optional UI resume failed: {error}");
    }
}
