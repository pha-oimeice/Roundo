//! Server admission for the test-Creature spawn intent.

use super::{
    SpawnTestCreatureController, SpawnTestCreatureControllerMessage, SpawnTestCreatureIntent,
};
use bevy::prelude::{MessageReader, MessageWriter, Query};

pub(crate) fn accept_spawn_test_creature(
    mut messages: MessageReader<SpawnTestCreatureControllerMessage>,
    mut controllers: Query<&mut SpawnTestCreatureController>,
    mut intents: MessageWriter<SpawnTestCreatureIntent>,
) {
    for message in messages.read() {
        let Ok(mut controller) = controllers.get_mut(message.entity) else {
            continue;
        };
        if controller.apply_command(message.command).is_ok() {
            intents.write(SpawnTestCreatureIntent {
                controller: message.entity,
            });
        }
    }
}
