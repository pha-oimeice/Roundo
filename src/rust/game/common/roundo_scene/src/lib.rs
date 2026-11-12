//! Bevy-native application flow and scene-instance composition.
//!
//! Flow lives in `State<Scene>`; a `SceneRoot` owns one instantiated data hierarchy.
//! Pausing changes flow without replacing the in-game hierarchy or stopping simulation.

use bevy::prelude::{Component, ComputedStates, States, bsn};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, States)]
pub enum Scene {
    #[default]
    Menu,
    InGame(GameplayState),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SceneKind {
    #[default]
    Menu,
    InGame,
}

impl ComputedStates for SceneKind {
    type SourceStates = Scene;
    // Running/Paused share one root lifetime, including state-scoped entities.
    const ALLOW_SAME_STATE_TRANSITIONS: bool = false;

    fn compute(scene: Scene) -> Option<Self> {
        Some(scene.kind())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

/// One concrete hierarchy, not a business scene number or network authority.
#[derive(Clone, Copy, Debug, Component)]
pub struct SceneRoot {
    pub kind: SceneKind,
}

pub fn scene_root(kind: SceneKind) -> impl bevy::scene::Scene {
    bsn! { ~{SceneRoot { kind }} }
}
