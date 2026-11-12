//! Native Bevy scene flow. UI construction succeeds before a transition is accepted.

mod runtime;
pub(crate) use runtime::reconcile_scene;
pub use runtime::{ClientScenePlugin, ClientSceneSystemSet};

use crate::network::ClientConnectionStatus;
use bevy::{prelude::*, state::state::StateTransition};
use roundo_scene::{GameplayState, Scene, SceneKind};
use roundo_ui::{UiCommandSource, UiInstanceId, UiLifecycleError, UiLifecycleManager};
use std::fmt;

#[derive(Clone, Copy, Debug)]
pub enum ClientUiIntent {
    Pause,
    Resume,
    Escape,
}

#[derive(Default, Resource)]
pub struct ClientSceneProjection {
    pub focused: Option<UiInstanceId>,
    pub world_active: bool,
    pub gameplay_input: bool,
}
#[derive(Component)]
struct PauseMenu;
#[derive(Debug)]
pub enum SceneTransitionError {
    NotRunning,
    NotPaused,
    PresentationNotReady,
    Ui(UiLifecycleError),
}
impl fmt::Display for SceneTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(f, "pause requires a running game Scene"),
            Self::NotPaused => write!(f, "resume requires a paused game Scene"),
            Self::PresentationNotReady => write!(f, "Scene presentation is not ready"),
            Self::Ui(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SceneTransitionError {}
fn current(ui: &UiLifecycleManager) -> Scene {
    *ui.world.resource::<State<Scene>>().get()
}
fn transition(ui: &mut UiLifecycleManager, next: Scene) {
    if current(ui) != next {
        ui.world.resource_mut::<NextState<Scene>>().set(next);
    }
}
/// Resource borrows must have returned before running OnEnter/OnExit systems.
pub fn apply_pending_transition(world: &mut World) {
    if !matches!(
        world.get_resource::<NextState<Scene>>(),
        Some(NextState::Pending(_) | NextState::PendingIfDifferent(_))
    ) {
        return;
    }
    let previous = *world.resource::<State<Scene>>().get();
    world.run_schedule(StateTransition);
    let next = *world.resource::<State<Scene>>().get();
    if previous != next {
        log::info!("Client Scene transition: {previous:?} -> {next:?}");
    }
}
fn pause_entity(ui: &mut UiLifecycleManager) -> Option<Entity> {
    let entities = ui
        .world
        .query_filtered::<Entity, With<PauseMenu>>()
        .iter(ui.world)
        .collect::<Vec<_>>();
    let entities = entities
        .into_iter()
        .filter(|e| ui.command_source_is_live(UiInstanceId::from_entity(*e)))
        .collect::<Vec<_>>();
    assert!(
        entities.len() <= 1,
        "only one pause presentation can be owned"
    );
    entities.into_iter().next()
}
pub fn synchronize(world: &mut World, status: &ClientConnectionStatus) {
    UiLifecycleManager::with(world, |ui| {
        if *status != ClientConnectionStatus::Connected {
            transition(ui, Scene::Menu);
        } else if current(ui) == Scene::Menu
            || (current(ui) == Scene::InGame(GameplayState::Paused) && pause_entity(ui).is_none())
        {
            transition(ui, Scene::InGame(GameplayState::Running));
        }
    });
    apply_pending_transition(world);
}
pub fn execute(
    intent: ClientUiIntent,
    ui: &mut UiLifecycleManager,
) -> Result<(), SceneTransitionError> {
    match intent {
        ClientUiIntent::Pause => pause(ui),
        ClientUiIntent::Resume => resume(ui),
        ClientUiIntent::Escape => {
            if let Some(focused) = ui.focused_instance() {
                if ui.is_root(focused) {
                    return Ok(());
                }
                if Some(focused.entity()) == pause_entity(ui) {
                    return resume(ui);
                }
                ui.back(focused).map_err(SceneTransitionError::Ui)
            } else {
                match current(ui) {
                    Scene::InGame(GameplayState::Paused) => resume(ui),
                    Scene::InGame(GameplayState::Running) if !ui.blocks_gameplay_input() => {
                        pause(ui)
                    }
                    _ => Ok(()),
                }
            }
        }
    }
}
pub fn projection(ui: &UiLifecycleManager, window_focused: bool) -> ClientSceneProjection {
    let state = current(ui);
    let in_game = state.kind() == SceneKind::InGame && ui.root_scene() == SceneKind::InGame;
    let focused = ui.focused_instance();
    ClientSceneProjection {
        focused,
        world_active: in_game && ui.world_visible(),
        gameplay_input: in_game
            && state.gameplay_input_allowed()
            && !ui.blocks_gameplay_input()
            && focused.is_none()
            && window_focused,
    }
}
fn pause(ui: &mut UiLifecycleManager) -> Result<(), SceneTransitionError> {
    if !current(ui).gameplay_input_allowed() {
        return Err(SceneTransitionError::NotRunning);
    }
    if ui.root_scene() != SceneKind::InGame {
        return Err(SceneTransitionError::PresentationNotReady);
    }
    let target = ui
        .resolve_slot("roundo.pause-menu")
        .map_err(SceneTransitionError::Ui)?;
    let instance = ui
        .navigate(UiCommandSource::Host, target)
        .map_err(SceneTransitionError::Ui)?;
    ui.world.entity_mut(instance.entity()).insert(PauseMenu);
    transition(ui, Scene::InGame(GameplayState::Paused));
    Ok(())
}
fn resume(ui: &mut UiLifecycleManager) -> Result<(), SceneTransitionError> {
    if current(ui) != Scene::InGame(GameplayState::Paused) {
        return Err(SceneTransitionError::NotPaused);
    }
    let entity = pause_entity(ui).ok_or(SceneTransitionError::NotPaused)?;
    ui.back(UiInstanceId::from_entity(entity))
        .map_err(SceneTransitionError::Ui)?;
    transition(ui, Scene::InGame(GameplayState::Running));
    Ok(())
}
