//! Mod 注册 Web UI 的门面 module。
//!
//! 这里只重导出调用方真正需要的 interface；Resource Type adapter、UI Lifecycle core、
//! Wry/Win32 平台 adapter 与 Bevy composition 各自保留私有 implementation。

mod composition;
mod lifecycle;
mod platform;
mod registry;

pub use composition::RoundoWebUiPlugin;
pub use lifecycle::{
    FocusedUiDeclaration, PendingUiDescriptor, RecoveryAction, RecoveryActionRequest,
    RecoverySurface, UiBounds, UiCommandSource, UiInstance, UiInstanceId, UiLifecycleError,
    UiLifecycleManager, UiLifecycleState, UiOpenTarget, UiRootCommit, UiRootReplacement,
};
pub use platform::UiNavigationExecutor;
pub use registry::{
    CONNECTED_ROOT_SLOT, ClientInteractionMode, DISCONNECTED_ROOT_SLOT, PresentationMode, UiImport,
    UiLayout, UiRegistry, UiRegistryError, UiResource, UiWorldVisibility,
};

#[cfg(all(test, target_os = "windows"))]
pub(crate) use platform::{
    StagedCommandGate, StagedReadiness, prepared_webview_readiness, should_start_document_load,
    staged_readiness,
};
#[cfg(test)]
pub(crate) use platform::{
    bridge_focus_request, bridge_handshake, frame_navigation_allowed, response_script,
    same_definition_path, top_level_navigation_allowed, webui_initialization_script,
};

#[cfg(test)]
mod tests;
