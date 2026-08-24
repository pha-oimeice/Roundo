use super::{
    BlockInteraction, BlockInteractionMessage, DestroyBlockController,
    DestroyBlockControllerMessage, PlaceBlockController, PlaceBlockControllerMessage,
};
use bevy::prelude::{MessageReader, MessageWriter, Query};

pub(crate) fn accept_block_interactions(
    mut destroy_messages: MessageReader<DestroyBlockControllerMessage>,
    mut place_messages: MessageReader<PlaceBlockControllerMessage>,
    mut destroy_controllers: Query<&mut DestroyBlockController>,
    mut place_controllers: Query<&mut PlaceBlockController>,
    mut interactions: MessageWriter<BlockInteractionMessage>,
) {
    for message in destroy_messages.read() {
        let Ok(mut controller) = destroy_controllers.get_mut(message.entity) else {
            continue;
        };
        if controller.accept(message.command).is_ok() {
            interactions.write(BlockInteractionMessage {
                entity: message.entity,
                interaction: BlockInteraction::Destroy,
            });
        }
    }
    for message in place_messages.read() {
        let Ok(mut controller) = place_controllers.get_mut(message.entity) else {
            continue;
        };
        let Ok(action) = controller.accept(message.command) else {
            continue;
        };
        interactions.write(BlockInteractionMessage {
            entity: message.entity,
            interaction: BlockInteraction::Place {
                voxel_id: action.voxel_id,
            },
        });
    }
}
