//! Client adapter for shared Scene rules. Mod UI actions are not Scene intents.

mod runtime;
pub use runtime::{ClientScenePlugin, ClientSceneSystemSet};

use crate::network::ClientConnectionStatus;
use bevy::prelude::Resource;
use roundo_scene::{GameplayState, Scene, SceneKind, SceneState};
use roundo_webui::{
    UiCommandSource, UiInstanceId, UiLifecycleError, UiLifecycleManager, UiNavigationExecutor,
    UiOpening,
};
use std::fmt;

#[derive(Clone, Copy, Debug)]
pub enum ClientUiIntent {
    Pause,
    Resume,
    Escape,
}

pub enum ClientUiOutcome {
    Opening {
        opening: UiOpening,
        resource: String,
    },
    Completed,
}

/// Hardware/presentation adapters consume this projection, not flow heuristics.
#[derive(Default, Resource)]
pub struct ClientSceneProjection {
    pub focused: Option<UiInstanceId>,
    pub world_active: bool,
    pub gameplay_input: bool,
}

#[derive(Default, Resource)]
pub struct ClientSceneManager {
    state: SceneState,
    pause_ui: Option<UiInstanceId>,
}

#[derive(Debug)]
pub enum SceneTransitionError {
    Rule(roundo_scene::SceneTransitionError),
    PresentationNotReady,
    Ui(UiLifecycleError),
}
impl fmt::Display for SceneTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rule(error) => error.fmt(f),
            Self::PresentationNotReady => write!(f, "Scene presentation is not ready"),
            Self::Ui(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SceneTransitionError {}

impl ClientSceneManager {
    pub fn synchronize(&mut self, status: &ClientConnectionStatus, ui: &UiLifecycleManager) {
        let previous = self.state.current();
        self.state
            .synchronize_session(*status == ClientConnectionStatus::Connected);
        self.pause_ui = self.pause_ui.filter(|id| ui.open_is_active(*id));
        if self.state.current() == Scene::Menu {
            self.pause_ui = None;
        } else if self.pause_ui.is_none()
            && self.state.current() == Scene::InGame(GameplayState::Paused)
        {
            self.state.resume().expect("a terminated pause can resume");
        }
        self.trace_transition(previous);
    }

    pub fn root_scene(&self) -> SceneKind {
        self.state.current().kind()
    }

    /// Accepts flow intents; physical input is interpreted by the input module.
    pub fn execute(
        &mut self,
        intent: ClientUiIntent,
        status: &ClientConnectionStatus,
        ui: &mut UiLifecycleManager,
        navigation: &mut UiNavigationExecutor,
    ) -> Result<ClientUiOutcome, SceneTransitionError> {
        self.synchronize(status, ui);
        let previous = self.state.current();
        let result = match intent {
            ClientUiIntent::Pause => self.pause(ui, navigation),
            ClientUiIntent::Resume => self.resume(ui, navigation),
            ClientUiIntent::Escape => {
                if ui.recovery_surface().is_some() {
                    return Ok(ClientUiOutcome::Completed);
                }
                if let Some(focused) = ui.focused_declaration() {
                    if ui.is_root_document(focused.instance) {
                        return Ok(ClientUiOutcome::Completed);
                    }
                    if Some(focused.instance) == self.pause_ui {
                        return self.resume(ui, navigation);
                    }
                    // Back is generic navigation, not a named Mod UI operation.
                    match navigation.back(ui, focused.instance) {
                        Ok(_) | Err(UiLifecycleError::CannotCloseUiRoot) => {
                            Ok(ClientUiOutcome::Completed)
                        }
                        Err(error) => Err(SceneTransitionError::Ui(error)),
                    }
                } else {
                    match self.state.current() {
                        Scene::Menu => Ok(ClientUiOutcome::Completed),
                        Scene::InGame(GameplayState::Paused) => self.resume(ui, navigation),
                        Scene::InGame(GameplayState::Running) if !ui.blocks_gameplay_input() => {
                            self.pause(ui, navigation)
                        }
                        Scene::InGame(GameplayState::Running) => Ok(ClientUiOutcome::Completed),
                    }
                }
            }
        };
        self.trace_transition(previous);
        result
    }

    pub fn projection(
        &self,
        ui: &UiLifecycleManager,
        navigation: &UiNavigationExecutor,
    ) -> ClientSceneProjection {
        let world_active =
            self.root_scene() == SceneKind::InGame && ui.root_scene() == SceneKind::InGame;
        let focused = ui.focused_declaration().map(|value| value.instance);
        ClientSceneProjection {
            focused,
            world_active,
            gameplay_input: world_active
                && self.state.current().gameplay_input_allowed()
                && !ui.blocks_gameplay_input()
                && focused.is_none()
                && ui.recovery_surface().is_none()
                && navigation.host_has_keyboard_focus(),
        }
    }

    fn pause(
        &mut self,
        ui: &mut UiLifecycleManager,
        navigation: &mut UiNavigationExecutor,
    ) -> Result<ClientUiOutcome, SceneTransitionError> {
        if !self.state.current().gameplay_input_allowed() {
            return Err(SceneTransitionError::Rule(
                roundo_scene::SceneTransitionError::NotRunning,
            ));
        }
        if ui.root_scene() != SceneKind::InGame || ui.recovery_surface().is_some() {
            return Err(SceneTransitionError::PresentationNotReady);
        }
        let target = ui
            .resolve_slot_path("roundo.pause-menu", None)
            .map_err(|error| {
                SceneTransitionError::Ui(UiLifecycleError::Registry(error.to_string()))
            })?;
        let resource = target.resource().to_owned();
        let opening = navigation
            .navigate(ui, UiCommandSource::Host, target)
            .map_err(SceneTransitionError::Ui)?;
        self.pause_ui = Some(opening.instance_id());
        self.state
            .pause()
            .expect("pause admission checked the same state");
        Ok(ClientUiOutcome::Opening { opening, resource })
    }

    fn resume(
        &mut self,
        ui: &mut UiLifecycleManager,
        navigation: &mut UiNavigationExecutor,
    ) -> Result<ClientUiOutcome, SceneTransitionError> {
        let id = self.pause_ui.ok_or(SceneTransitionError::Rule(
            roundo_scene::SceneTransitionError::NotPaused,
        ))?;
        if !ui.command_source_is_live(id) {
            return Err(SceneTransitionError::PresentationNotReady);
        }
        navigation.back(ui, id).map_err(SceneTransitionError::Ui)?;
        self.pause_ui = None;
        self.state
            .resume()
            .expect("an owned pause belongs to Paused");
        Ok(ClientUiOutcome::Completed)
    }

    fn trace_transition(&self, previous: Scene) {
        let next = self.state.current();
        if previous != next {
            log::info!("Client Scene transition: {previous:?} -> {next:?}");
        }
    }
}
