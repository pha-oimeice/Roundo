//! Maps local mouse input to sequenced block-interaction commands.

use super::{DestroyBlockControllerAction, PlaceBlockControllerAction};
use crate::character_controller::{
    ClientInputBindings, DESTROY_BLOCK_INPUT_SLOT, PLACE_BLOCK_INPUT_SLOT,
    client::{
        AuthoritativePlayerState, ClientControllerCommandState, ClientMarionetteEvent,
        ClientPipeResource, ClientPlacedVoxelId, ClientPlayerController,
    },
};
use bevy::prelude::{ButtonInput, KeyCode, MouseButton, Res, ResMut};
use roundo_contracts::PlayerControllerCommand;

/// Emits interactions only while an authoritative player accepts local input.
pub(crate) fn route_block_interactions(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    bindings: Res<ClientInputBindings>,
    controller: Res<ClientPlayerController>,
    placed_voxel: Option<Res<ClientPlacedVoxelId>>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
) {
    if !controller.input_enabled || controller.spirit_walking || authoritative.0.is_none() {
        return;
    }

    // Each press issues one sequence number; failed bridge delivery is observable.
    if bindings.just_pressed(DESTROY_BLOCK_INPUT_SLOT, &keyboard, &mouse_buttons)
        && let Ok(command) = controller_commands
            .destroy_block
            .issue(DestroyBlockControllerAction)
    {
        if pipe
            .0
            .try_send(ClientMarionetteEvent::UsePlayerController(
                PlayerControllerCommand::DestroyBlock(command),
            ))
            .is_err()
        {
            log::warn!("cannot route destroy-block input: reason=client_bridge_closed");
        }
    }
    // Placement captures the currently selected voxel at command creation time.
    if bindings.just_pressed(PLACE_BLOCK_INPUT_SLOT, &keyboard, &mouse_buttons)
        && let Ok(command) = controller_commands
            .place_block
            .issue(PlaceBlockControllerAction {
                voxel_id: placed_voxel.as_deref().copied().unwrap_or_default().0,
            })
    {
        if pipe
            .0
            .try_send(ClientMarionetteEvent::UsePlayerController(
                PlayerControllerCommand::PlaceBlock(command),
            ))
            .is_err()
        {
            log::warn!("cannot route place-block input: reason=client_bridge_closed");
        }
    }
}
