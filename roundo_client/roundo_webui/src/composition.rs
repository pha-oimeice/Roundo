//! Bevy composition 门面。
//!
//! Composition 只装配 Registry、Lifecycle core 与平台 adapter。

use crate::{
    RecoveryActionRequest, UiCommandSource, UiLifecycleManager, UiNavigationExecutor, UiRegistry,
};
use bevy::prelude::{App, IntoScheduleConfigs, Plugin};
use roundo_mod_loader::LoadedMods;
use roundo_toolbox::request_response_pipe::ContextualJsonRequestResponseIo;
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

/// Web UI 的 Bevy composition 门面。
pub struct RoundoWebUiPlugin {
    mods_root: Option<PathBuf>,
    loaded_mods: Option<LoadedMods>,
    registry: Option<UiRegistry>,
    command_io: Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
}
impl RoundoWebUiPlugin {
    pub fn from_current_dir() -> Self {
        Self {
            mods_root: Some(
                std::env::current_dir()
                    .expect("current directory is unavailable")
                    .join("mods"),
            ),
            loaded_mods: None,
            registry: None,
            command_io: None,
        }
    }
    pub fn from_current_dir_with_command_io(
        command_io: ContextualJsonRequestResponseIo<UiCommandSource, Value>,
    ) -> Self {
        Self {
            mods_root: Some(
                std::env::current_dir()
                    .expect("current directory is unavailable")
                    .join("mods"),
            ),
            loaded_mods: None,
            registry: None,
            command_io: Some(command_io),
        }
    }
    pub fn with_mods_root(path: impl Into<PathBuf>) -> Self {
        Self {
            mods_root: Some(path.into()),
            loaded_mods: None,
            registry: None,
            command_io: None,
        }
    }
    pub fn with_mods_root_and_command_io(
        path: impl Into<PathBuf>,
        command_io: ContextualJsonRequestResponseIo<UiCommandSource, Value>,
    ) -> Self {
        Self {
            mods_root: Some(path.into()),
            loaded_mods: None,
            registry: None,
            command_io: Some(command_io),
        }
    }

    /// Reuses the composition root's validated Mod set so independently owned
    /// Resource Type adapters cannot discover different installations.
    pub fn with_loaded_mods_and_command_io(
        loaded_mods: LoadedMods,
        command_io: ContextualJsonRequestResponseIo<UiCommandSource, Value>,
    ) -> Self {
        Self {
            mods_root: None,
            loaded_mods: Some(loaded_mods),
            registry: None,
            command_io: Some(command_io),
        }
    }

    /// Consumes the Web UI Resource Type already published by the process-wide
    /// loading phase; this composition adapter performs no resource discovery.
    pub fn with_registry_and_command_io(
        registry: UiRegistry,
        command_io: ContextualJsonRequestResponseIo<UiCommandSource, Value>,
    ) -> Self {
        Self {
            mods_root: None,
            loaded_mods: None,
            registry: Some(registry),
            command_io: Some(command_io),
        }
    }
}
impl Plugin for RoundoWebUiPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_os = "windows")]
        app.insert_resource(bevy_winit::WinitSettings {
            focused_mode: bevy_winit::UpdateMode::reactive(Duration::from_millis(16)),
            unfocused_mode: bevy_winit::UpdateMode::reactive(Duration::from_millis(16)),
        });
        let registry = if let Some(registry) = self.registry.as_ref() {
            registry.clone()
        } else {
            let discovered;
            let mods = if let Some(mods) = self.loaded_mods.as_ref() {
                mods
            } else {
                let root = self
                    .mods_root
                    .as_ref()
                    .expect("Web UI needs a published registry, loaded Mods, or a Mod root");
                discovered = LoadedMods::discover(root).unwrap_or_else(|error| {
                    panic!("cannot load Mods from {}: {error}", root.display())
                });
                &discovered
            };
            UiRegistry::load(mods)
                .unwrap_or_else(|error| panic!("cannot load Web UI registry: {error}"))
        };
        let mut manager = UiLifecycleManager::new(registry);
        #[cfg(target_os = "windows")]
        manager.begin_configured_root().unwrap_or_else(|error| {
            panic!("cannot stage configured disconnected Root UI: {error}")
        });
        #[cfg(not(target_os = "windows"))]
        manager
            .open_configured_root()
            .unwrap_or_else(|error| panic!("cannot open configured disconnected Root UI: {error}"));
        app.insert_resource(manager);
        app.add_message::<RecoveryActionRequest>();
        app.insert_non_send(UiNavigationExecutor::default());
        #[cfg(target_os = "windows")]
        {
            app.insert_non_send(crate::platform::WebUiCommandEndpoint {
                io: self.command_io.clone(),
            });
            app.add_systems(
                bevy::app::Update,
                (
                    crate::platform::retire_superseded_staged_webviews,
                    crate::platform::stage_pending_webview,
                    crate::platform::advance_staged_webviews,
                    crate::platform::schedule_graph_prefetches,
                    crate::platform::sync_recovery_surface,
                    crate::platform::sync_committed_navigation,
                    crate::platform::resize_webview,
                    crate::platform::apply_windows_input_mode,
                    crate::platform::resolve_webui_commands,
                )
                    .chain(),
            );
        }
    }
}
