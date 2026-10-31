//! Authoritative acceptance and temporary fixed-tick movement application.

use super::{AcceptedMovementIntent, Movement3D, Movement3DMessage};
use bevy::prelude::{MessageReader, MessageWriter, Query};

/// Accepts the newest valid movement intent for each controlled entity.
pub(crate) fn accept_movement_commands(
    mut messages: MessageReader<Movement3DMessage>,
    mut controllers: Query<&mut Movement3D>,
    mut accepted_intents: MessageWriter<AcceptedMovementIntent>,
) {
    for message in messages.read() {
        let Ok(mut controller) = controllers.get_mut(message.entity) else {
            continue;
        };
        if let Ok(action) = controller.apply_command(message.command) {
            accepted_intents.write(AcceptedMovementIntent {
                controller: message.entity,
                local_axis: action.direction,
                sequence: message.command.sequence,
            });
        }
    }
}
