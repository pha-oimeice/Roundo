//! Server-side controller admission, compaction, and fixed-tick routing.

use super::{
    PlayerControllerCommand,
    movement::{Movement3DAction, Movement3DMessage, accept_movement_commands},
    rotation::{AcceptedGazeIntent, GazeController, GazeIntentMessage},
    test_creature::{
        SpawnTestCreatureControllerMessage, SpawnTestCreatureIntent, accept_spawn_test_creature,
    },
};
use bevy::prelude::{
    App, Component, Entity, FixedUpdate, IntoScheduleConfigs, MessageWriter, Plugin, Query, Res,
    Resource, SystemSet,
};
pub use roundo_contracts::ConnectionId;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{BTreeMap, HashMap};

// The fixed budget bounds work when a client floods controller commands.
const MAX_CONTROL_COMMANDS_PER_TICK: usize = 4096;

/// Network-facing endpoint for authoritative controller commands.
pub type ServerMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ServerMarionetteCommand, ServerMarionetteEvent>;

#[derive(Clone)]
/// Installs controller routing and authoritative application systems.
pub struct MarionetteServerPlugin {
    pipe: CrossbeamThreadPipe<ServerMarionetteCommand, ServerMarionetteEvent>,
}

impl MarionetteServerPlugin {
    /// Creates a plugin with a fresh bidirectional command pipe.
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    /// Returns a clonable endpoint for the network adapter.
    pub fn ipc(&self) -> ServerMarionetteIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for MarionetteServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// Commands are admitted before controllers mutate transforms or world state.
impl Plugin for MarionetteServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ServerPipeResource(self.pipe.endpoint_b()))
            .insert_resource(ServerMarionetteSnapshotSender(self.pipe.endpoint_b()))
            .add_message::<Movement3DMessage>()
            .add_message::<super::AcceptedMovementIntent>()
            .add_message::<GazeIntentMessage>()
            .add_message::<AcceptedGazeIntent>()
            .add_message::<SpawnTestCreatureControllerMessage>()
            .add_message::<SpawnTestCreatureIntent>()
            .configure_sets(
                FixedUpdate,
                (
                    MarionetteServerSet::Commands,
                    MarionetteServerSet::Controllers,
                    MarionetteServerSet::Movement,
                    MarionetteServerSet::BlockInteractions,
                )
                    .chain(),
            )
            .add_systems(
                FixedUpdate,
                process_controller_commands.in_set(MarionetteServerSet::Commands),
            )
            .add_systems(
                FixedUpdate,
                (
                    accept_movement_commands,
                    accept_gaze_commands,
                    accept_spawn_test_creature,
                )
                    .in_set(MarionetteServerSet::Controllers),
            );
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
/// Fixed-update ordering boundaries for controller processing.
pub enum MarionetteServerSet {
    Commands,
    Controllers,
    Movement,
    BlockInteractions,
}

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
/// Associates a player entity with its owning network connection.
pub struct NetworkControllerTarget {
    pub connection_id: ConnectionId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// Controller intent received from an authenticated connection.
pub enum ServerMarionetteEvent {
    CreatureMotionSnapshot {
        connection_id: ConnectionId,
        snapshot: roundo_contracts::CreatureMotionSnapshot,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ServerMarionetteCommand {
    UsePlayerController {
        connection_id: ConnectionId,
        command: PlayerControllerCommand,
    },
}

#[derive(Resource, Clone)]
/// ECS-side endpoint consumed during fixed updates.
struct ServerPipeResource(
    CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
);

/// Composition adapter sends committed opaque Creature snapshots through this endpoint.
#[derive(Resource, Clone)]
pub struct ServerMarionetteSnapshotSender(
    pub CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ServerMarionetteEvent>,
);

// Resolves connection ownership and compacts commands before simulation.
fn process_controller_commands(
    pipe: Res<ServerPipeResource>,
    targets: Query<(Entity, &NetworkControllerTarget)>,
    movements: Query<&super::Movement3D>,
    mut movement_messages: MessageWriter<Movement3DMessage>,
    gazes: Query<&GazeController>,
    mut gaze_messages: MessageWriter<GazeIntentMessage>,
    mut spawn_test_creature_messages: MessageWriter<SpawnTestCreatureControllerMessage>,
) {
    let targets = targets
        .iter()
        .map(|(entity, target)| (target.connection_id, entity))
        .collect::<HashMap<_, _>>();
    // Movement intent is latest-wins while retaining monotonic sequencing.
    let mut movement_by_entity =
        HashMap::<Entity, roundo_contracts::ControllerCommand<Movement3DAction>>::new();
    // Gaze commands are deltas, so every unacknowledged sequence contributes.
    // Grouping by sequence deduplicates retries and makes the same-tick sum
    // independent of arrival order while acknowledging the highest sequence.
    let mut gaze_by_entity = HashMap::<Entity, BTreeMap<u64, super::GazeIntent>>::new();

    for _ in 0..MAX_CONTROL_COMMANDS_PER_TICK {
        let Some(command) = pipe.0.try_receive() else {
            break;
        };
        let ServerMarionetteCommand::UsePlayerController {
            connection_id,
            command,
        } = command;
        let Some(&entity) = targets.get(&connection_id) else {
            continue;
        };
        match command {
            PlayerControllerCommand::SetGaze(_) => { /* Never accept absolute look through legacy untargeted IPC. */
            }
            PlayerControllerCommand::Movement3D(command) => {
                let movement = movements.get(entity);
                let accepted_movement_sequence =
                    movement.map_or(0, super::Movement3D::last_accepted_sequence);
                if command.sequence <= accepted_movement_sequence {
                    continue;
                }
                movement_by_entity
                    .entry(entity)
                    .and_modify(|pending: &mut roundo_contracts::ControllerCommand<_>| {
                        if command.sequence > pending.sequence {
                            *pending = command;
                        }
                    })
                    .or_insert(command);
            }
            PlayerControllerCommand::Gaze(command) => {
                if gazes.get(entity).map_or(true, |gaze| {
                    command.sequence > gaze.last_accepted_sequence()
                }) && command.action.yaw_delta.is_finite()
                    && command.action.pitch_delta.is_finite()
                {
                    gaze_by_entity
                        .entry(entity)
                        .or_default()
                        .insert(command.sequence, command.action);
                }
            }
            PlayerControllerCommand::SpawnTestCreature(command) => {
                spawn_test_creature_messages
                    .write(SpawnTestCreatureControllerMessage { entity, command });
            }
        }
    }

    for (entity, command) in movement_by_entity {
        let _movement_message = movement_messages.write(Movement3DMessage { entity, command });
    }
    for (entity, commands) in gaze_by_entity {
        let Some((&sequence, _)) = commands.last_key_value() else {
            continue;
        };
        let (yaw_delta, pitch_delta) = commands.values().fold((0.0_f32, 0.0_f32), |sum, intent| {
            (sum.0 + intent.yaw_delta, sum.1 + intent.pitch_delta)
        });
        if yaw_delta.is_finite() && pitch_delta.is_finite() {
            let _gaze_message = gaze_messages.write(GazeIntentMessage {
                entity,
                command: roundo_contracts::ControllerCommand {
                    sequence,
                    action: super::GazeIntent {
                        yaw_delta,
                        pitch_delta,
                    },
                },
            });
        }
    }
}

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
