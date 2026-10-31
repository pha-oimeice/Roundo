//! Shared game-flow model and transition rules, independent of UI and networking.
//!
//! Each runtime owns its SceneState and supplies authoritative session facts.
//! Sharing types does not replicate state or let a Client pause a Server world.

use std::fmt;

/// Source-defined flow; Mod resources only present this state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Scene {
    /// No established gameplay session, including connection attempts and errors.
    #[default]
    Menu,
    InGame(GameplayState),
}

/// Stable root identity. In-game state changes retain the same world/UI root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SceneKind {
    #[default]
    Menu,
    InGame,
}

/// Paused suppresses interaction; it does not imply stopping world simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameplayState {
    Running,
    Paused,
}

impl Scene {
    pub fn kind(self) -> SceneKind {
        match self {
            Self::Menu => SceneKind::Menu,
            Self::InGame(_) => SceneKind::InGame,
        }
    }

    pub fn gameplay_input_allowed(self) -> bool {
        self == Self::InGame(GameplayState::Running)
    }
}

/// Pure transition authority. Platform readiness and side effects belong to adapters.
#[derive(Debug, Default)]
pub struct SceneState {
    current: Scene,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneTransitionError {
    NotRunning,
    NotPaused,
}

impl fmt::Display for SceneTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(f, "pause requires a running game Scene"),
            Self::NotPaused => write!(f, "resume requires a paused game Scene"),
        }
    }
}
impl std::error::Error for SceneTransitionError {}

impl SceneState {
    pub fn current(&self) -> Scene {
        self.current
    }

    /// Session loss clears all game state. An unchanged active session preserves pause.
    pub fn synchronize_session(&mut self, active: bool) {
        self.current = match (active, self.current) {
            (false, _) => Scene::Menu,
            (true, Scene::Menu) => Scene::InGame(GameplayState::Running),
            (true, current) => current,
        };
    }

    /// Call only after the adapter accepts its pause effects. Failure is non-mutating.
    pub fn pause(&mut self) -> Result<(), SceneTransitionError> {
        if !self.current.gameplay_input_allowed() {
            return Err(SceneTransitionError::NotRunning);
        }
        self.current = Scene::InGame(GameplayState::Paused);
        Ok(())
    }

    /// Call after accepted resume effects, or when the pause presentation terminates.
    pub fn resume(&mut self) -> Result<(), SceneTransitionError> {
        if self.current != Scene::InGame(GameplayState::Paused) {
            return Err(SceneTransitionError::NotPaused);
        }
        self.current = Scene::InGame(GameplayState::Running);
        Ok(())
    }
}
