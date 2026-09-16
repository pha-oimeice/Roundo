//! Movement intent, validation, and client/server routing.

mod client;
mod server;

use super::shared::{ControllerAction, EventController};
use bevy::prelude::{Component, Message, Vec3};
use roundo_contracts::ControllerCommand;
pub use roundo_contracts::Movement3DAction;

#[cfg(test)]
pub(super) use self::client::movement_world_direction;
pub(super) use self::client::{move_spirit_camera, route_player_movement};
pub(super) use self::server::{accept_movement_commands, apply_movement};

/// Temporary authoritative speed until the kinematic system owns movement.
pub const PLAYER_MOVE_SPEED: f32 = 5.0;

/// Authoritative movement controller and latest accepted direction.
#[derive(Component, Clone, Debug, Default)]
pub struct Movement3D {
    commands: EventController<Movement3DAction>,
    direction: Vec3,
}

impl Movement3D {
    pub fn issue(
        &mut self,
        action: Movement3DAction,
    ) -> Result<ControllerCommand<Movement3DAction>, super::ControllerError> {
        let command = self.commands.issue(action)?;
        self.direction = Vec3::from_array(action.direction).normalize_or_zero();
        Ok(command)
    }

    pub fn apply_command(
        &mut self,
        command: ControllerCommand<Movement3DAction>,
    ) -> Result<Movement3DAction, super::ControllerError> {
        let action = self.commands.apply_command(command)?;
        self.direction = Vec3::from_array(action.direction).normalize_or_zero();
        Ok(action)
    }

    pub(crate) fn last_accepted_sequence(&self) -> u64 {
        self.commands.last_accepted_sequence()
    }

    pub(crate) fn direction(&self) -> Vec3 {
        self.direction
    }
}

impl ControllerAction for Movement3DAction {
    fn is_valid(self) -> bool {
        let direction = Vec3::from_array(self.direction);
        direction.is_finite() && direction.length_squared() <= 1.0 + f32::EPSILON
    }
}

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct Movement3DMessage {
    pub entity: bevy::prelude::Entity,
    pub command: ControllerCommand<Movement3DAction>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_rejects_non_finite_oversized_and_stale_directions() {
        let mut movement = Movement3D::default();
        assert!(
            movement
                .issue(Movement3DAction {
                    direction: [f32::NAN, 0.0, 0.0],
                })
                .is_err()
        );
        assert!(
            movement
                .issue(Movement3DAction {
                    direction: [2.0, 0.0, 0.0],
                })
                .is_err()
        );
        let accepted = movement.apply_command(ControllerCommand {
            sequence: 2,
            action: Movement3DAction {
                direction: [1.0, 0.0, 0.0],
            },
        });
        assert!(accepted.is_ok());
        let stale = movement.apply_command(ControllerCommand {
            sequence: 1,
            action: Movement3DAction {
                direction: [0.0, 1.0, 0.0],
            },
        });
        assert!(stale.is_err());
    }
}
