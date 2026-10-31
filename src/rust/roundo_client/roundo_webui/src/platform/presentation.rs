//! Physical presentation and verified Close. No logical lifetime decisions.

use super::*;
use crate::state::{NativeFact, RecoveryId, UiDocumentProof, UiPresentation, UiViewLifetime};
use std::sync::atomic::AtomicU64;
use wry::{WebViewBuilderExtWindows, WebViewExtWindows};

pub(super) fn webview_rect(bounds: UiBounds) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(bounds.x, bounds.y).into(),
        size: wry::dpi::LogicalSize::new(bounds.width, bounds.height).into(),
    }
}

pub(crate) fn resize_webview(
    windows: bevy::prelude::Query<
        &bevy::window::Window,
        (
            bevy::ecs::query::With<bevy::window::PrimaryWindow>,
            bevy::ecs::query::Changed<bevy::window::Window>,
        ),
    >,
    mut manager: bevy::prelude::ResMut<UiLifecycleManager>,
) {
    if let Some(window) = windows.iter().next() {
        manager.set_client_bounds(window.width() as u32, window.height() as u32);
    }
}

pub(super) fn apply_presentation(
    view: &mut NativeView,
    desired: UiPresentation,
    proof: Option<UiDocumentProof>,
) -> Result<bool, String> {
    if proof.is_some()
        && view
            .observation
            .confirmed_document()
            .map(|(current, _)| current)
            != proof
    {
        return Err("presentation document proof became stale".into());
    }
    let previous = view.last_presentation;
    if previous.is_none_or(|p| p.bounds != desired.bounds) {
        view.webview
            .set_bounds(webview_rect(desired.bounds))
            .map_err(|e| format!("presentation bounds: {e}"))?;
    }
    if previous.is_none_or(|p| p.interaction_mode != desired.interaction_mode) {
        apply_windows_webview_mode(&view.webview, desired.interaction_mode)
            .map_err(|error| format!("presentation input mode: {error}"))?;
    }
    // Each call can reenter native callbacks. Never acknowledge an old proof
    // after such work, even when the logical Open identity did not change.
    if proof.is_some()
        && view
            .observation
            .confirmed_document()
            .map(|(current, _)| current)
            != proof
    {
        return Err("presentation changed document during bounds/input work".into());
    }
    let visibility_changed = previous.is_none_or(|p| p.visible != desired.visible);
    let focus_changed = previous.is_none_or(|p| p.focused != desired.focused);
    if visibility_changed {
        view.webview
            .set_visible(desired.visible)
            .map_err(|e| format!("presentation visibility: {e}"))?;
        dispatch_lifecycle_event(
            &view.webview,
            &view.scripts,
            "roundo:visibility",
            if desired.visible { "visible" } else { "hidden" },
        )
        .map_err(|e| e.to_string())?;
    }
    if focus_changed {
        dispatch_lifecycle_event(
            &view.webview,
            &view.scripts,
            "roundo:focus",
            if desired.focused {
                "focused"
            } else {
                "blurred"
            },
        )
        .map_err(|e| e.to_string())?;
    }
    if (visibility_changed && desired.visible) || (focus_changed && desired.focused) {
        raise_webview(&view.webview).map_err(|error| format!("presentation z-order: {error}"))?;
    }
    if previous != Some(desired) {
        invalidate_webview_parent(&view.webview)
            .map_err(|error| format!("presentation invalidation: {error}"))?;
    }
    if desired.host_focused {
        use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};
        // SAFETY: live owned child on its UI thread; the host outlives this view.
        let parent = unsafe { GetAncestor(view.webview.hwnd(), GA_ROOT) };
        // Focus is a handoff effect, not a window-style change. Reapply and
        // verify it even for a forced presentation with an unchanged HUD mode,
        // after visibility/Z-order work and before acknowledging retirement.
        focus_host(parent).map_err(|error| format!("presentation host focus: {error}"))?;
    }
    if proof.is_some()
        && view
            .observation
            .confirmed_document()
            .map(|(current, _)| current)
            != proof
    {
        return Err("presentation changed document before acknowledgement".into());
    }
    view.last_presentation = Some(desired);
    Ok(desired.visible)
}

pub(super) fn dispatch_lifecycle_event(
    webview: &wry::WebView,
    scripts: &ScriptTracker,
    name: &str,
    state: &str,
) -> wry::Result<()> {
    let name = serde_json::to_string(name).expect("event name serializes");
    let state = serde_json::to_string(state).expect("event state serializes");
    evaluate_script_transaction(
        webview,
        scripts,
        &format!("window.dispatchEvent(new CustomEvent({name},{{detail:{{state:{state}}}}}));"),
    )
}

pub(super) fn close_view(
    view: &mut NativeView,
    callbacks: &HostCallbackActivity,
) -> Result<(), String> {
    close_native_owner(&view.webview, &mut view.handlers, &view.scripts, callbacks)
}

pub(super) fn close_native_owner(
    webview: &wry::WebView,
    handlers: &mut driver::NativeHandlerTokens,
    scripts: &ScriptTracker,
    callbacks: &HostCallbackActivity,
) -> Result<(), String> {
    if callbacks.depth() != 0 || scripts.pending() != 0 {
        return Err("physical Close requested without fresh native quiescence".into());
    }
    driver::remove_host_navigation_handlers(&webview.webview(), handlers)?;
    // SAFETY: the caller retains the live Wry owner on the UI apartment; its
    // explicit host handlers have been detached, and no callback/script is live.
    unsafe { webview.controller().Close() }
        .map_err(|e| format!("WebView2 controller Close failed: {e}"))?;
    if callbacks.depth() != 0 || scripts.pending() != 0 {
        return Err("native work remained after Close; retaining physical owner".into());
    }
    Ok(())
}

fn raise_webview(webview: &wry::WebView) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    // SAFETY: owned live child HWND, UI thread; preserve geometry and activation.
    unsafe {
        SetWindowPos(
            webview.hwnd(),
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
    }
    .map_err(|e| e.to_string())
}
fn invalidate_webview_parent(webview: &wry::WebView) -> Result<(), String> {
    use windows::Win32::{
        Graphics::Gdi::InvalidateRect,
        UI::WindowsAndMessaging::{GA_ROOT, GetAncestor},
    };
    // SAFETY: the live Wry child and its parent remain owned during this UI call.
    let parent = unsafe { GetAncestor(webview.hwnd(), GA_ROOT) };
    if parent.is_invalid() {
        return Err("WebView parent HWND is unavailable".into());
    }
    // SAFETY: validated live parent HWND; no borrowed callback state is held.
    unsafe { InvalidateRect(Some(parent), None, false) }
        .ok()
        .map_err(|e| e.to_string())
}
fn apply_windows_webview_mode(
    webview: &wry::WebView,
    mode: ClientInteractionMode,
) -> Result<(), String> {
    use windows::Win32::{
        Foundation::{GetLastError, SetLastError, WIN32_ERROR},
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, SetWindowLongW, WS_DISABLED, WS_EX_NOACTIVATE,
            WS_EX_TRANSPARENT,
        },
    };
    let hwnd = webview.hwnd();
    // SAFETY: valid owned child on its UI thread. These functions return the
    // previous value, so inspect LastError only for an ambiguous zero return.
    // Do not send reentrant style notifications when the bits already match.
    unsafe {
        for (index, bits) in [
            (
                GWL_EXSTYLE,
                (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) as i32,
            ),
            (GWL_STYLE, WS_DISABLED.0 as i32),
        ] {
            SetLastError(WIN32_ERROR(0));
            let previous = GetWindowLongW(hwnd, index);
            if previous == 0 {
                GetLastError().ok().map_err(|e| e.to_string())?;
            }
            let next = if mode == ClientInteractionMode::InGame {
                previous | bits
            } else {
                previous & !bits
            };
            if next == previous {
                continue;
            }
            SetLastError(WIN32_ERROR(0));
            if SetWindowLongW(hwnd, index, next) == 0 {
                GetLastError().ok().map_err(|e| e.to_string())?;
            }
        }
    }
    // Styles do not select the keyboard focus owner. Presentation applies that
    // separate effect after visibility work; hidden/background HUDs cannot steal
    // focus from an interactive UI. No SWP_FRAMECHANGED or child activation.
    Ok(())
}

fn host_hwnd(context: &NativeContext) -> Result<windows::Win32::Foundation::HWND, String> {
    use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    bevy_winit::WINIT_WINDOWS.with(|all| {
        let all = all.borrow();
        let window = context
            .parent
            .and_then(|entity| all.get_window(entity))
            .ok_or_else(|| "host parent unavailable".to_owned())?;
        match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::Win32(handle) => {
                Ok(windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _))
            }
            _ => Err("host is not a Win32 window".into()),
        }
    })
}
pub(super) fn host_has_keyboard_focus(context: &NativeContext) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
    host_hwnd(context).is_ok_and(|parent| {
        // SAFETY: sampled on the UI thread; comparing handles does not borrow
        // the host or transfer focus. A missing host is handled above.
        unsafe { GetFocus() == parent }
    })
}

fn focus_host(parent: windows::Win32::Foundation::HWND) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    if parent.is_invalid() {
        return Err("host focus HWND is unavailable".into());
    }
    // SAFETY: live host HWND sampled from the owned window, same UI thread.
    // SetFocus returns the PREVIOUS HWND, which can legitimately be null. The
    // windows binding treats that as Err; verify the actual postcondition.
    let previous = unsafe { SetFocus(Some(parent)) };
    if unsafe { GetFocus() } == parent {
        Ok(())
    } else {
        Err(previous.err().map_or_else(
            || "host focus handoff was not applied".into(),
            |error| error.to_string(),
        ))
    }
}
pub(super) fn present_world(context: &NativeContext) -> Result<(), String> {
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    let parent = host_hwnd(context)?;
    focus_host(parent)?;
    // SAFETY: same live host HWND on its owning UI thread; confirm paint
    // invalidation before acknowledging the handoff, without a visual guess.
    unsafe { InvalidateRect(Some(parent), None, false) }
        .ok()
        .map_err(|e| e.to_string())
}

const RECOVERY_HTML: &str = r#"<!doctype html><html data-roundo-recovery="ready"><meta charset="utf-8"><title>Roundo UI recovery</title><style>html,body{margin:0;width:100%;height:100%;background:#111;color:#eee;font:16px sans-serif}main{height:100%;display:grid;place-content:center;text-align:center;gap:16px}button{padding:10px 20px;margin:4px}</style><main><h1>UI failed to load</h1><p>The game state is unchanged.</p><div><button onclick="send('retry')">Retry</button><button onclick="send('disconnect')">Disconnect</button><button onclick="send('quit')">Quit</button></div></main><script>function send(recovery_action){window.ipc.postMessage(JSON.stringify({recovery_action}))}</script></html>"#;

fn host_bounds(context: &NativeContext) -> Option<UiBounds> {
    bevy_winit::WINIT_WINDOWS.with(|all| {
        let all = all.borrow();
        let window = context.parent.and_then(|entity| all.get_window(entity))?;
        let size = window.inner_size();
        let scale = window.scale_factor();
        Some(UiBounds {
            x: 0,
            y: 0,
            width: (size.width as f64 / scale).round() as u32,
            height: (size.height as f64 / scale).round() as u32,
        })
    })
}

pub(super) struct RecoveryView {
    bounds: UiBounds,
    pub(super) webview: wry::WebView,
    pub(super) handlers: driver::NativeHandlerTokens,
    pub(super) scripts: ScriptTracker,
    pub(super) observation: NativeObservation,
    actions: Arc<Mutex<Vec<RecoveryAction>>>,
    probe: Option<UiDocumentProof>,
    ready: Arc<Mutex<Option<UiDocumentProof>>>,
    presented: Option<RecoveryId>,
}
impl WryDriver {
    pub(super) fn build_recovery(&mut self, _id: RecoveryId) -> Result<(), String> {
        let observation = NativeObservation::new("__roundo_host_recovery__", "");
        let token = observation.token();
        let scripts = ScriptTracker::new(self.callbacks.clone(), token);
        let actions = Arc::new(Mutex::new(Vec::new()));
        let ipc_actions = actions.clone();
        let ipc_callbacks = self.callbacks.clone();
        let wake = self.context.wake.clone();
        let bounds = host_bounds(&self.context)
            .ok_or_else(|| "Recovery parent bounds unavailable".to_owned())?;
        let webview = bevy_winit::WINIT_WINDOWS.with(|all| {
            let all = all.borrow();
            let window = self
                .context
                .parent
                .and_then(|entity| all.get_window(entity))
                .ok_or_else(|| "Recovery parent unavailable".to_owned())?;
            // Recovery is application UI too, with no browser action entry points.
            wry::WebViewBuilder::new()
                .with_devtools(false)
                .with_default_context_menus(false)
                .with_browser_accelerator_keys(false)
                .with_hotkeys_zoom(false)
                .with_visible(false)
                .with_url("about:blank")
                .with_bounds(webview_rect(bounds))
                .with_navigation_handler(|url| url == "about:blank")
                .with_ipc_handler(move |message| {
                    let _guard = ipc_callbacks.enter_for(token);
                    let action = serde_json::from_str::<Value>(message.body())
                        .ok()
                        .and_then(|v| match v.get("recovery_action").and_then(Value::as_str) {
                            Some("retry") => Some(RecoveryAction::Retry),
                            Some("disconnect") => Some(RecoveryAction::Disconnect),
                            Some("quit") => Some(RecoveryAction::Quit),
                            _ => None,
                        });
                    if let Some(action) = action {
                        let mut pending = ipc_actions.lock().unwrap_or_else(|p| p.into_inner());
                        if pending.len() < 64 {
                            pending.push(action);
                        }
                        drop(pending);
                        wake();
                    }
                })
                .build_as_child(&**window)
                .map_err(|e| e.to_string())
        })?;
        let install = driver::install_host_navigation_handlers(
            &webview.webview(),
            driver::HostNavigationHandlerInputs {
                observation: observation.clone(),
                callbacks: self.callbacks.clone(),
                view_token: token,
                frame_definition: "__roundo_host_recovery__".into(),
                wake: self.context.wake.clone(),
                subscriptions: Arc::new(Mutex::new(BTreeSet::new())),
                subscription_generation: Arc::new(AtomicU64::new(0)),
                command_gate: None,
                pending_commands: None,
            },
        );
        self.recovery = Some(RecoveryView {
            bounds,
            webview,
            handlers: install.tokens,
            scripts,
            observation,
            actions,
            probe: None,
            ready: Arc::new(Mutex::new(None)),
            presented: None,
        });
        install.result?;
        self.recovery
            .as_ref()
            .expect("stored physical owner")
            .webview
            .load_html(RECOVERY_HTML)
            .map_err(|e| e.to_string())
    }
    pub(super) fn poll_recovery(&mut self, lifetime: &UiViewLifetime, facts: &mut Vec<NativeFact>) {
        let Some(view) = &mut self.recovery else {
            return;
        };
        if let Some(bounds) = host_bounds(&self.context).filter(|bounds| *bounds != view.bounds) {
            match view.webview.set_bounds(webview_rect(bounds)) {
                Ok(()) => view.bounds = bounds,
                Err(error) => facts.push(NativeFact::NativeFault {
                    message: format!("cannot resize Recovery Surface: {error}"),
                }),
            }
        }
        if let Some(Err(message)) = view.observation.snapshot().navigation_result {
            facts.push(NativeFact::NativeFault {
                message: format!("Recovery navigation failed: {message}"),
            });
        }
        self.recovery_actions.extend(std::mem::take(
            &mut *view.actions.lock().unwrap_or_else(|p| p.into_inner()),
        ));
        let drained = view.observation.drain();
        if let Some(message) = drained.fault {
            facts.push(NativeFact::NativeFault { message });
        }
        if let Some(proof) = view
            .observation
            .loaded_document()
            .filter(|proof| Some(*proof) != view.probe)
        {
            view.probe = Some(proof);
            let ticket = view.scripts.begin();
            let completed = ticket.clone();
            let callbacks = view.scripts.clone();
            let observation = view.observation.clone();
            let ready = view.ready.clone();
            let wake = self.context.wake.clone();
            let result = view.webview.evaluate_script_with_callback(
                "document.documentElement.dataset.roundoRecovery === 'ready'",
                move |result| {
                    let _guard = callbacks.enter_callback();
                    completed.complete();
                    if result == "true" && observation.loaded_document() == Some(proof) {
                        *ready.lock().unwrap_or_else(|p| p.into_inner()) = Some(proof);
                    }
                    wake();
                },
            );
            if let Err(error) = result {
                ticket.complete();
                facts.push(NativeFact::NativeFault {
                    message: format!("Recovery readiness probe failed: {error}"),
                });
            }
        }
        let Some((id, _)) = lifetime.recovery_request() else {
            return;
        };
        if view.presented == Some(id) {
            return;
        }
        let ready = *view.ready.lock().unwrap_or_else(|p| p.into_inner());
        if ready.is_none() || view.observation.loaded_document() != ready {
            return;
        }
        let result = (|| {
            apply_windows_webview_mode(&view.webview, ClientInteractionMode::WebUi)?;
            view.webview.set_visible(true).map_err(|e| e.to_string())?;
            raise_webview(&view.webview)?;
            invalidate_webview_parent(&view.webview)
        })();
        match result {
            Ok(()) => {
                if view.observation.loaded_document() == ready {
                    view.presented = Some(id);
                    facts.push(NativeFact::RecoveryPresented { recovery: id });
                }
            }
            Err(message) => facts.push(NativeFact::NativeFault { message }),
        }
    }
}
