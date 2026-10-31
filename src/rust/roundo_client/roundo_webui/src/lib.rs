//! Mod 注册 Web UI 的门面 module。
//!
//! 这里只重导出调用方真正需要的 interface；Resource Type adapter、UI Lifecycle core、
//! Wry/Win32 平台 adapter 与 Bevy composition 各自保留私有 implementation。

mod composition;
mod lifecycle;
mod platform;
mod registry;
mod state;

pub use composition::{RoundoWebUiPlugin, UiHostDocumentScript, WebUiSystemSet};
pub use lifecycle::{
    FocusedUiDeclaration, PendingUiDescriptor, RecoveryAction, RecoveryActionRequest,
    RecoverySurface, UiBounds, UiCommandSource, UiInstance, UiInstanceId, UiLifecycleError,
    UiLifecycleManager, UiOpenTarget, UiRootCommit, UiRootReplacement,
};
pub use platform::{UiNavigationExecutor, UiOpening};
pub use registry::{
    ClientInteractionMode, IN_GAME_ROOT_SLOT, MENU_ROOT_SLOT, PresentationMode, UiImport, UiLayout,
    UiRegistry, UiRegistryError, UiResource, UiWorldVisibility,
};
