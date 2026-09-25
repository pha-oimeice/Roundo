//! Converts a registered semantic input slot into a sequenced test-spawn intent.

use super::SpawnTestCreatureAction;
use crate::character_controller::{
    ClientInputBindings, SPAWN_TEST_CREATURE_INPUT_SLOT,
    client::{
        AuthoritativePlayerState, ClientControllerCommandState, ClientMarionetteEvent,
        ClientPipeResource, ClientPlayerController,
    },
};
use bevy::prelude::{ButtonInput, KeyCode, MouseButton, Res, ResMut};
use roundo_contracts::PlayerControllerCommand;

pub(crate) fn route_spawn_test_creature(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    bindings: Res<ClientInputBindings>,
    controller: Res<ClientPlayerController>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut commands: ResMut<ClientControllerCommandState>,
) {
    if !controller.input_enabled || controller.spirit_walking || authoritative.0.is_none() {
        return;
    }
    if bindings.just_pressed(SPAWN_TEST_CREATURE_INPUT_SLOT, &keyboard, &mouse_buttons)
        && let Ok(command) = commands.spawn_test_creature.issue(SpawnTestCreatureAction)
        && pipe
            .0
            .try_send(ClientMarionetteEvent::UsePlayerController(
                PlayerControllerCommand::SpawnTestCreature(command),
            ))
            .is_err()
    {
        log::warn!("cannot route spawn-test-creature input: reason=client_bridge_closed");
    }
}
