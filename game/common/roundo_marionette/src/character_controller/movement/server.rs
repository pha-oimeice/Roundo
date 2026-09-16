//! Authoritative acceptance and temporary fixed-tick movement application.

use super::{Movement3D, Movement3DMessage, PLAYER_MOVE_SPEED};
use bevy::prelude::{MessageReader, Query, Res, Time, Transform};

/// Accepts the newest valid movement intent for each controlled entity.
pub(crate) fn accept_movement_commands(
    mut messages: MessageReader<Movement3DMessage>,
    mut controllers: Query<&mut Movement3D>,
) {
    for message in messages.read() {
        let Ok(mut controller) = controllers.get_mut(message.entity) else {
            continue;
        };
        let _accepted = controller.apply_command(message.command);
    }
}

/// Temporary movement implementation pending the authoritative kinematic system.
pub(crate) fn apply_movement(
    time: Res<Time<bevy::prelude::Fixed>>,
    mut controllers: Query<(&Movement3D, &mut Transform)>,
) {
    let delta_seconds = time.delta_secs();
    for (controller, mut transform) in &mut controllers {
        transform.translation += controller.direction() * PLAYER_MOVE_SPEED * delta_seconds;
    }
}
