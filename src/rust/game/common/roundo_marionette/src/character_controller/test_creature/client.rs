//! Converts a registered semantic input slot into a sequenced test-spawn intent.

use super::SpawnTestCreatureAction;
use crate::character_controller::{
    ClientInputFrame, SPAWN_TEST_CREATURE_INPUT_SLOT,
    client::{
        AuthoritativePlayerState, ClientControllerCommandState, ClientMarionetteEvent,
        ClientPipeResource, ClientPlayerController,
    },
};
use bevy::prelude::{MessageReader, Res, ResMut};
use roundo_contracts::PlayerControllerCommand;

pub(crate) fn route_spawn_test_creature(
    mut inputs: MessageReader<ClientInputFrame>,
    controller: Res<ClientPlayerController>,
    authoritative: Res<AuthoritativePlayerState>,
    pipe: Res<ClientPipeResource>,
    mut commands: ResMut<ClientControllerCommandState>,
) {
    let input = inputs.read().last().cloned().unwrap_or_default();
    if !controller.input_enabled || controller.spirit_walking || authoritative.0.is_none() {
        return;
    }
    if input.just_pressed(SPAWN_TEST_CREATURE_INPUT_SLOT)
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
