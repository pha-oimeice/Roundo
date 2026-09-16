//! Server validation and routing for block-interaction commands.

use super::{
    BlockInteraction, BlockInteractionMessage, DestroyBlockController,
    DestroyBlockControllerMessage, PlaceBlockController, PlaceBlockControllerMessage,
};
use bevy::prelude::{MessageReader, MessageWriter, Query};

/// Converts accepted controller commands into authoritative interaction messages.
pub(crate) fn accept_block_interactions(
    mut destroy_messages: MessageReader<DestroyBlockControllerMessage>,
    mut place_messages: MessageReader<PlaceBlockControllerMessage>,
    mut destroy_controllers: Query<&mut DestroyBlockController>,
    mut place_controllers: Query<&mut PlaceBlockController>,
    mut interactions: MessageWriter<BlockInteractionMessage>,
) {
    // Destruction carries no payload beyond its sequencing metadata.
    let pending_destroy_messages = destroy_messages.read();
    for message in pending_destroy_messages {
        let Ok(mut controller) = destroy_controllers.get_mut(message.entity) else {
            continue;
        };
        let accepted = controller.apply_command(message.command);
        if accepted.is_ok() {
            let _interaction_message = interactions.write(BlockInteractionMessage {
                entity: message.entity,
                interaction: BlockInteraction::Destroy,
            });
        }
    }
    // Placement forwards only the voxel ID validated by the controller action.
    let pending_place_messages = place_messages.read();
    for message in pending_place_messages {
        let Ok(mut controller) = place_controllers.get_mut(message.entity) else {
            continue;
        };
        let Ok(action) = controller.apply_command(message.command) else {
            continue;
        };
        let _interaction_message = interactions.write(BlockInteractionMessage {
            entity: message.entity,
            interaction: BlockInteraction::Place {
                voxel_id: action.voxel_id,
            },
        });
    }
}
