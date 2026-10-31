//! Navigation facade and physical adapters around the one document lifetime.
//! Logical Lifecycle Tree authority remains in UiLifecycleManager.

mod command_bridge;
#[cfg(target_os = "windows")]
mod driver;
#[cfg(target_os = "windows")]
mod native;
#[cfg(target_os = "windows")]
mod native_observation;
#[cfg(target_os = "windows")]
mod presentation;
#[cfg(target_os = "windows")]
mod protocol;
#[cfg(target_os = "windows")]
mod runtime;
#[cfg(target_os = "windows")]
mod staging;
#[cfg(target_os = "windows")]
pub(crate) use native::StagedCommandGate;
#[cfg(target_os = "windows")]
use native::*;

#[cfg(target_os = "windows")]
pub(crate) use command_bridge::resolve_webui_commands;
use command_bridge::*;
pub(crate) use command_bridge::{
    bridge_focus_request, bridge_handshake, data_sync_script, webui_initialization_script,
};
#[cfg(target_os = "windows")]
pub(crate) use native::drive_native_views;
#[cfg(target_os = "windows")]
use native_observation::*;
#[cfg(target_os = "windows")]
pub(crate) use presentation::resize_webview;
#[cfg(target_os = "windows")]
use presentation::*;
#[cfg(target_os = "windows")]
use protocol::protocol_response;

use crate::state::{UiOpeningId, UiViewLifetime};
use crate::*;
use roundo_scene::SceneKind;
use roundo_toolbox::request_response_pipe::{
    ContextualJsonRequestResponseIo, JsonSubmitError, RequestCall, ResponsePoll, ResponseSender,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The exact opening accepted by one navigation executor.
///
/// This host-local, non-cloneable token is not proof of a committed document or
/// command authority. Consuming it to transfer a response does not complete the
/// opening: Windows still waits for actual document readiness or failure. Host
/// navigation may drop the token without cancelling its accepted work.
#[derive(Debug, Eq, PartialEq)]
pub struct UiOpening {
    id: UiOpeningId,
}

impl UiOpening {
    /// Returns the pending/future instance identity, not a live-source capability.
    pub fn instance_id(&self) -> UiInstanceId {
        self.id.instance_id()
    }
}

/// Owns document transactions and, on Windows, the UI-thread physical adapter.
/// Pending identities grant no command authority; only confirmed document
/// transactions can commit them. Root replacement revokes old authority without
/// assuming that its native resources can already be dropped.
pub struct UiNavigationExecutor {
    lifetime: UiViewLifetime,
    #[cfg(target_os = "windows")]
    native: native::WryDriver,
}
impl Default for UiNavigationExecutor {
    fn default() -> Self {
        Self {
            lifetime: UiViewLifetime::new(Duration::from_secs(15)),
            #[cfg(target_os = "windows")]
            native: native::WryDriver::default(),
        }
    }
}
impl UiNavigationExecutor {
    pub fn set_load_timeout(&mut self, timeout: Duration) {
        self.lifetime.set_timeout(timeout);
    }

    /// Accepts navigation and returns its exact response-handoff token.
    /// A later or failed navigation cannot retarget that token to another opening.
    pub fn navigate(
        &mut self,
        state: &mut UiLifecycleManager,
        source: UiCommandSource,
        target: UiOpenTarget,
    ) -> Result<UiOpening, UiLifecycleError> {
        #[cfg(target_os = "windows")]
        let id = self.lifetime.open(state, source, target, Instant::now())?;
        #[cfg(not(target_os = "windows"))]
        let id = {
            let instance = state.open_immediately(source, target)?;
            self.lifetime.opening(instance.value())
        };
        Ok(UiOpening { id })
    }

    /// Consumes an exact opening token and transfers its response at most once.
    ///
    /// The token must originate from this executor. Windows rejects an opening
    /// that is already committed, terminal, or response-bound; non-Windows has
    /// already committed logically and responds immediately. Failure returns the
    /// sender and value unchanged. This operation never performs native work.
    pub fn complete_open_command(
        &mut self,
        opening: UiOpening,
        sender: ResponseSender<Value>,
        success: Value,
    ) -> Result<(), (ResponseSender<Value>, Value)> {
        #[cfg(target_os = "windows")]
        {
            self.lifetime
                .bind_open_response(opening.id, sender, success)
        }
        #[cfg(not(target_os = "windows"))]
        {
            if !self.lifetime.owns_opening(opening.id) {
                return Err((sender, success));
            }
            let _ = sender.respond(success);
            Ok(())
        }
    }

    pub fn open_configured_root(
        &mut self,
        state: &mut UiLifecycleManager,
    ) -> Result<Option<UiInstanceId>, UiLifecycleError> {
        #[cfg(target_os = "windows")]
        {
            self.lifetime.open_configured_root(state, Instant::now())
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
        let result = self.lifetime.back(state, source);
        self.revoke_stale();
        result
    }
    pub fn unload_definitions(
        &mut self,
        state: &mut UiLifecycleManager,
        definitions: impl IntoIterator<Item = String>,
    ) -> Result<Vec<UiInstanceId>, UiLifecycleError> {
        let result = self.lifetime.unload_definitions(state, definitions);
        self.revoke_stale();
        result
    }
    pub fn replace_root(
        &mut self,
        state: &mut UiLifecycleManager,
        scene: SceneKind,
    ) -> Result<UiRootReplacement, UiLifecycleError> {
        let result = self.lifetime.replace_root(state, scene, Instant::now());
        self.revoke_stale();
        result
    }
    fn revoke_stale(&mut self) {
        #[cfg(target_os = "windows")]
        {
            use runtime::ViewDriver;
            self.native.revoke_stale(&self.lifetime);
        }
    }

    /// Samples whether the native host, rather than a child WebView, currently
    /// owns keyboard focus. This does not request focus or grant game authority.
    /// Windows returns false until the host is available; later native work or
    /// OS focus changes invalidate the sample. Non-Windows retains logical-only
    /// adapter behavior and returns true.
    pub fn host_has_keyboard_focus(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            presentation::host_has_keyboard_focus(&self.native.context)
        }
        #[cfg(not(target_os = "windows"))]
        {
            true
        }
    }

    /// Drains source-qualified document payloads. The framework does not interpret
    /// application payloads; old documents and retired instances are discarded.
    pub fn take_document_messages(&mut self) -> Vec<(UiInstanceId, Value)> {
        #[cfg(target_os = "windows")]
        {
            self.native.take_document_messages(&self.lifetime)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Vec::new()
        }
    }

    /// Delivers a transient event to one exact, still-qualified subscribed UI.
    /// Unlike Client Data, events are not cached or replayed to new listeners.
    pub fn publish_event(&mut self, target: UiInstanceId, resource: &str, data: &Value) -> usize {
        #[cfg(target_os = "windows")]
        {
            self.native
                .publish_event(&self.lifetime, target, resource, data)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (target, resource, data);
            0
        }
    }

    pub fn data_subscription_generation(&self) -> u64 {
        #[cfg(target_os = "windows")]
        {
            return self
                .native
                .subscription_generation
                .load(std::sync::atomic::Ordering::Acquire);
        }
        #[cfg(not(target_os = "windows"))]
        0
    }
    pub fn has_data_subscribers(&self, resource: &str) -> bool {
        #[cfg(target_os = "windows")]
        {
            return self.native.live_views(&self.lifetime).any(|view| {
                view.subscriptions
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains(resource)
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = resource;
            false
        }
    }
    /// Pushes only to still-live, currently qualified documents. Native script
    /// callbacks remain tracked even if logical removal follows this call.
    pub fn publish_data(&mut self, resource: &str, data: &Value) -> usize {
        #[cfg(target_os = "windows")]
        {
            return self.native.publish_data(&self.lifetime, resource, data);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (resource, data);
            0
        }
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
fn webview2_resource_url(resource: &str, path: &str) -> String {
    format!("http://roundo-ui.resource/{resource}/{path}")
}
pub(crate) fn top_level_navigation_allowed(source_resource: &str, url: &str) -> bool {
    same_definition_path(source_resource, url).is_some()
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
