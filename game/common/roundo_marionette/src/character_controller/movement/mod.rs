mod client;
mod server;

use super::shared::{ControllerAction, EventController};
use bevy::prelude::{ButtonInput, KeyCode, Message, Vec3};
use roundo_networking::protocol::ControllerCommand;
pub use roundo_networking::protocol::Movement3DAction;
use std::collections::BTreeMap;

#[cfg(test)]
pub(super) use self::client::movement_world_direction;
pub(super) use self::client::{move_spirit_camera, route_player_movement};
pub(super) use self::server::apply_movement;

pub type Movement3D = EventController<Movement3DAction>;

impl ControllerAction for Movement3DAction {
    fn is_valid(self) -> bool {
        self.translation_delta.iter().all(|value| value.is_finite())
    }
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct Movement3DMessage {
    pub entity: bevy::prelude::Entity,
    pub command: ControllerCommand<Movement3DAction>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MovementAction {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    MoveForward,
    MoveBackward,
}

impl MovementAction {
    pub const ALL: [Self; 6] = [
        Self::MoveUp,
        Self::MoveDown,
        Self::MoveLeft,
        Self::MoveRight,
        Self::MoveForward,
        Self::MoveBackward,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::MoveUp => "Move Up",
            Self::MoveDown => "Move Down",
            Self::MoveLeft => "Move Left",
            Self::MoveRight => "Move Right",
            Self::MoveForward => "Move Forward",
            Self::MoveBackward => "Move Backward",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::MoveUp => 0,
            Self::MoveDown => 1,
            Self::MoveLeft => 2,
            Self::MoveRight => 3,
            Self::MoveForward => 4,
            Self::MoveBackward => 5,
        }
    }

    const fn direction(self) -> Vec3 {
        match self {
            Self::MoveUp => Vec3::Y,
            Self::MoveDown => Vec3::NEG_Y,
            Self::MoveLeft => Vec3::NEG_X,
            Self::MoveRight => Vec3::X,
            Self::MoveForward => Vec3::Z,
            Self::MoveBackward => Vec3::NEG_Z,
        }
    }
}

#[derive(bevy::prelude::Resource, Clone, Debug, Eq, PartialEq)]
pub struct ClientKeyBindings {
    bindings: BTreeMap<KeyCode, Vec<MovementAction>>,
}

impl ClientKeyBindings {
    pub fn empty() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (KeyCode, &[MovementAction])> {
        self.bindings
            .iter()
            .map(|(key, actions)| (*key, actions.as_slice()))
    }

    pub fn actions_for(&self, key: KeyCode) -> &[MovementAction] {
        self.bindings.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn keys_for(&self, action: MovementAction) -> Vec<KeyCode> {
        self.bindings
            .iter()
            .filter_map(|(key, actions)| actions.contains(&action).then_some(*key))
            .collect()
    }

    pub fn bind(&mut self, key: KeyCode, action: MovementAction) -> bool {
        let actions = self.bindings.entry(key).or_default();
        if actions.contains(&action) {
            return false;
        }
        actions.push(action);
        true
    }

    pub fn unbind(&mut self, key: KeyCode, action: MovementAction) -> bool {
        let Some(actions) = self.bindings.get_mut(&key) else {
            return false;
        };
        let Some(index) = actions.iter().position(|bound| *bound == action) else {
            return false;
        };
        actions.remove(index);
        if actions.is_empty() {
            self.bindings.remove(&key);
        }
        true
    }

    pub fn reorder(
        &mut self,
        key: KeyCode,
        action: MovementAction,
        target: MovementAction,
    ) -> bool {
        let Some(actions) = self.bindings.get_mut(&key) else {
            return false;
        };
        let Some(from) = actions.iter().position(|bound| *bound == action) else {
            return false;
        };
        let Some(to) = actions.iter().position(|bound| *bound == target) else {
            return false;
        };
        if from == to {
            return false;
        }
        let action = actions.remove(from);
        actions.insert(to.min(actions.len()), action);
        true
    }

    fn movement_direction(&self, keyboard: &ButtonInput<KeyCode>) -> Vec3 {
        let mut direction = Vec3::ZERO;
        for (key, actions) in &self.bindings {
            if !keyboard.pressed(*key) {
                continue;
            }
            for action in actions {
                direction += action.direction();
            }
        }
        direction.clamp(Vec3::NEG_ONE, Vec3::ONE)
    }
}

impl Default for ClientKeyBindings {
    fn default() -> Self {
        let mut bindings = Self::empty();
        bindings.bind(KeyCode::Space, MovementAction::MoveUp);
        bindings.bind(KeyCode::ShiftLeft, MovementAction::MoveDown);
        bindings.bind(KeyCode::KeyA, MovementAction::MoveLeft);
        bindings.bind(KeyCode::KeyD, MovementAction::MoveRight);
        bindings.bind(KeyCode::KeyW, MovementAction::MoveForward);
        bindings.bind(KeyCode::KeyS, MovementAction::MoveBackward);
        bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roundo_networking::protocol::ControllerCommand;

    #[test]
    fn movement_rejects_non_finite_deltas_and_stale_sequences() {
        let mut movement = Movement3D::default();
        assert!(
            movement
                .issue(Movement3DAction {
                    translation_delta: [f32::NAN, 0.0, 0.0],
                })
                .is_err()
        );
        assert!(
            movement
                .accept(ControllerCommand {
                    sequence: 2,
                    action: Movement3DAction {
                        translation_delta: [1.0, 2.0, 3.0],
                    },
                })
                .is_ok()
        );
        assert!(
            movement
                .accept(ControllerCommand {
                    sequence: 1,
                    action: Movement3DAction {
                        translation_delta: [4.0, 5.0, 6.0],
                    },
                })
                .is_err()
        );
    }
}
