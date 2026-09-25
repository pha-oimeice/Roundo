//! Server-side controller admission, compaction, and fixed-tick routing.

use super::{
    PlayerControllerCommand,
    block_interaction::{
        BlockInteractionMessage, DestroyBlockControllerMessage, PlaceBlockControllerMessage,
        accept_block_interactions,
    },
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
            .add_message::<DestroyBlockControllerMessage>()
            .add_message::<PlaceBlockControllerMessage>()
            .add_message::<BlockInteractionMessage>()
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
                    accept_block_interactions,
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
    mut destroy_block_messages: MessageWriter<DestroyBlockControllerMessage>,
    mut place_block_messages: MessageWriter<PlaceBlockControllerMessage>,
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
            PlayerControllerCommand::DestroyBlock(command) => {
                let message = DestroyBlockControllerMessage { entity, command };
                let _destroy_message = destroy_block_messages.write(message);
            }
            PlayerControllerCommand::PlaceBlock(command) => {
                let message = PlaceBlockControllerMessage { entity, command };
                let _place_message = place_block_messages.write(message);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControllerCommand, GazeIntent, Movement3D, Movement3DAction, PlayerControllers};
    use bevy::prelude::{App, Quat, Time, Transform, Vec3};
    use std::time::Duration;

    #[test]
    fn server_speed_applies_direction_without_using_rotation() {
        let plugin = MarionetteServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time<bevy::prelude::Fixed>>()
            .add_plugins(plugin);
        let entity = app
            .world_mut()
            .spawn((
                PlayerControllers::default(),
                NetworkControllerTarget {
                    connection_id: ConnectionId(7),
                },
                Transform::from_rotation(Quat::from_rotation_y(1.0)),
            ))
            .id();

        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(7),
            command: PlayerControllerCommand::Movement3D(ControllerCommand {
                sequence: 1,
                action: Movement3DAction {
                    direction: [1.0, 0.0, 0.0],
                },
            }),
        })
        .unwrap();
        app.world_mut()
            .resource_mut::<Time<bevy::prelude::Fixed>>()
            .advance_by(Duration::from_secs_f32(0.2));
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::ZERO
        );
        assert_eq!(
            app.world().get::<Movement3D>(entity).unwrap().direction(),
            Vec3::X
        );
    }

    #[test]
    fn gaze_intent_does_not_set_controller_transform() {
        let plugin = MarionetteServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time<bevy::prelude::Fixed>>()
            .add_plugins(plugin);
        let entity = app
            .world_mut()
            .spawn((
                GazeController::default(),
                NetworkControllerTarget {
                    connection_id: ConnectionId(8),
                },
                Transform::default(),
            ))
            .id();

        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(8),
            command: PlayerControllerCommand::Gaze(ControllerCommand {
                sequence: 1,
                action: GazeIntent {
                    yaw_delta: 0.25,
                    pitch_delta: 0.0,
                },
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);

        assert!(
            app.world()
                .get::<Transform>(entity)
                .unwrap()
                .rotation
                .abs_diff_eq(Quat::IDENTITY, f32::EPSILON)
        );
        assert!(app.world().get::<Movement3D>(entity).is_none());
    }

    #[test]
    fn queued_movement_is_compacted_before_simulation() {
        let plugin = MarionetteServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time<bevy::prelude::Fixed>>()
            .add_plugins(plugin);
        let entity = app
            .world_mut()
            .spawn((
                Movement3D::default(),
                NetworkControllerTarget {
                    connection_id: ConnectionId(10),
                },
                Transform::default(),
            ))
            .id();

        for sequence in 1..=100 {
            ipc.try_send(ServerMarionetteCommand::UsePlayerController {
                connection_id: ConnectionId(10),
                command: PlayerControllerCommand::Movement3D(ControllerCommand {
                    sequence,
                    action: Movement3DAction {
                        direction: [0.01, 0.0, 0.0],
                    },
                }),
            })
            .unwrap();
        }
        app.world_mut()
            .resource_mut::<Time<bevy::prelude::Fixed>>()
            .advance_by(Duration::from_secs_f32(0.2));
        app.world_mut().run_schedule(FixedUpdate);

        assert!(
            app.world()
                .get::<Transform>(entity)
                .unwrap()
                .translation
                .abs_diff_eq(Vec3::ZERO, 0.0001)
        );
        assert_eq!(
            app.world()
                .get::<Movement3D>(entity)
                .unwrap()
                .last_accepted_sequence(),
            100
        );
    }

    #[test]
    fn stale_movement_does_not_change_translation() {
        let plugin = MarionetteServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time<bevy::prelude::Fixed>>()
            .add_plugins(plugin);
        let entity = app
            .world_mut()
            .spawn((
                Movement3D::default(),
                NetworkControllerTarget {
                    connection_id: ConnectionId(9),
                },
                Transform::default(),
            ))
            .id();

        for sequence in [2, 1] {
            ipc.try_send(ServerMarionetteCommand::UsePlayerController {
                connection_id: ConnectionId(9),
                command: PlayerControllerCommand::Movement3D(ControllerCommand {
                    sequence,
                    action: Movement3DAction {
                        direction: [1.0, 0.0, 0.0],
                    },
                }),
            })
            .unwrap();
        }
        app.world_mut()
            .resource_mut::<Time<bevy::prelude::Fixed>>()
            .advance_by(Duration::from_secs_f32(0.2));
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::ZERO
        );
    }
}
