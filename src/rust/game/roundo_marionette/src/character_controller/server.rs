//! Local controller systems turn validated Bevy messages into gameplay intents.
use super::{
    movement::{Movement3DMessage, accept_movement_commands},
    rotation::{AcceptedGazeIntent, GazeController, GazeIntentMessage},
    test_creature::{
        SpawnTestCreatureControllerMessage, SpawnTestCreatureIntent, accept_spawn_test_creature,
    },
};
use bevy::prelude::{
    App, FixedUpdate, IntoScheduleConfigs, MessageWriter, Plugin, Query, SystemSet,
};

pub struct MarionetteServerPlugin;

impl Plugin for MarionetteServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Movement3DMessage>()
            .add_message::<super::AcceptedMovementIntent>()
            .add_message::<GazeIntentMessage>()
            .add_message::<AcceptedGazeIntent>()
            .add_message::<SpawnTestCreatureControllerMessage>()
            .add_message::<SpawnTestCreatureIntent>()
            .add_systems(
                FixedUpdate,
                (
                    accept_movement_commands,
                    accept_gaze_commands,
                    accept_spawn_test_creature,
                )
                    .in_set(MarionetteServerSet),
            );
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct MarionetteServerSet;

fn accept_gaze_commands(
    mut messages: bevy::prelude::MessageReader<GazeIntentMessage>,
    mut controllers: Query<&mut GazeController>,
    mut accepted: MessageWriter<AcceptedGazeIntent>,
) {
    for message in messages.read() {
        if let Ok(mut controller) = controllers.get_mut(message.entity)
            && controller.accept(message.command.sequence, message.command.action)
        {
            accepted.write(AcceptedGazeIntent {
                controller: message.entity,
                sequence: message.command.sequence,
                yaw_delta: message.command.action.yaw_delta,
                pitch_delta: message.command.action.pitch_delta,
            });
        }
    }
}
