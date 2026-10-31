//! Routes block triggers only after the current gaze enters the shared FIFO.
use crate::character_controller::{
    ClientInputFrame, DESTROY_BLOCK_INPUT_SLOT, PLACE_BLOCK_INPUT_SLOT,
    client::{
        ClientControllerCommandState, ClientMarionetteEvent, ClientPipeResource,
        ClientPlayerController, ClientPlayerControllerAccess, LocallyRoutedControllerIntent,
    },
    rotation::{
        ClientRotationSyncState, RotationUploadMode, RotationUploadOutcome,
        synchronize_current_rotation,
    },
};
use bevy::prelude::*;
use roundo_contracts::{ControllerCommand, DirectedControllerInput, ItemUseIntent, ItemUseMethod};

pub(crate) fn route_gaze_and_block_interactions(
    mut inputs: MessageReader<ClientInputFrame>,
    controller: Res<ClientPlayerController>,
    access: Res<ClientPlayerControllerAccess>,
    pipe: Res<ClientPipeResource>,
    mut commands: ResMut<ClientControllerCommandState>,
    time: Res<Time>,
    mut rotation_sync: ResMut<ClientRotationSyncState>,
    mut routed_intents: MessageWriter<LocallyRoutedControllerIntent>,
    cameras: Query<&Transform, With<Camera>>,
) {
    let input = inputs.read().last().cloned().unwrap_or_default();
    let destroy_edge = input.just_pressed(DESTROY_BLOCK_INPUT_SLOT);
    let place_edge = input.just_pressed(PLACE_BLOCK_INPUT_SLOT);
    let interaction = destroy_edge || place_edge;
    let mode = if interaction {
        RotationUploadMode::Flush
    } else {
        RotationUploadMode::RateLimited
    };
    let outcome = synchronize_current_rotation(
        &time,
        &controller,
        &access,
        &pipe,
        &mut rotation_sync,
        mode,
        &mut routed_intents,
        &cameras,
    );
    if !interaction || outcome != RotationUploadOutcome::Submitted {
        return;
    }
    let Some(id) =
        access.producing_controller(roundo_contracts::ControllerIntentDomain::BlockInteraction)
    else {
        return;
    };
    let Some(item) = access.snapshot().and_then(|snapshot| snapshot.block_item) else {
        return;
    };
    let methods = [
        destroy_edge.then_some(ItemUseMethod::DestroyBlock),
        place_edge.then_some(ItemUseMethod::PlaceBlock),
    ];
    for method in methods.into_iter().flatten() {
        let Some(sequence) = commands.block_sequence.checked_add(1) else {
            return;
        };
        let input = DirectedControllerInput::UseItem(ControllerCommand {
            sequence,
            action: ItemUseIntent { item, method },
        });
        if pipe
            .0
            .try_send(ClientMarionetteEvent::SubmitControllerInput {
                controller_id: id,
                input,
            })
            .is_err()
        {
            log::warn!("cannot route block interaction: reason=client_bridge_closed");
            return;
        }
        commands.block_sequence = sequence;
    }
}
