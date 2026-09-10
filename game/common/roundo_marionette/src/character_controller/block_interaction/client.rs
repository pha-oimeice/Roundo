use super::{DestroyBlockControllerAction, PlaceBlockControllerAction};
use crate::character_controller::client::{
    AuthoritativePlayerState, ClientControllerCommandState, ClientMarionetteEvent,
    ClientPipeResource, ClientPlayerController,
};
use bevy::prelude::{ButtonInput, MouseButton, Res, ResMut};
use roundo_contracts::PlayerControllerCommand;

const DEFAULT_PLACED_VOXEL_ID: u32 = 1;

pub(crate) fn route_block_interactions(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    controller: Res<ClientPlayerController>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut controller_commands: ResMut<ClientControllerCommandState>,
) {
    if !controller.input_enabled || controller.spirit_walking || authoritative.0.is_none() {
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left)
        && let Ok(command) = controller_commands
            .destroy_block
            .issue(DestroyBlockControllerAction)
    {
        let _ = pipe.0.try_send(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::DestroyBlock(command),
        ));
    }
    if mouse_buttons.just_pressed(MouseButton::Right)
        && let Ok(command) = controller_commands
            .place_block
            .issue(PlaceBlockControllerAction {
                voxel_id: DEFAULT_PLACED_VOXEL_ID,
            })
    {
        let _ = pipe.0.try_send(ClientMarionetteEvent::UsePlayerController(
            PlayerControllerCommand::PlaceBlock(command),
        ));
    }
}
