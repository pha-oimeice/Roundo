use super::{
    PlayerControllerCommand,
    block_interaction::{
        BlockInteractionMessage, DestroyBlockControllerMessage, PlaceBlockControllerMessage,
        accept_block_interactions,
    },
    movement::{Movement3DAction, Movement3DMessage, apply_movement},
    rotation::{RotationSyncMessage, apply_rotation_sync},
};
use bevy::prelude::{
    App, Component, Entity, FixedUpdate, IntoScheduleConfigs, MessageWriter, Plugin, Query, Res,
    Resource, SystemSet,
};
pub use roundo_networking::ConnectionId;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::HashMap;

const MAX_CONTROL_COMMANDS_PER_TICK: usize = 4096;

pub type ServerMarionetteIpc = CrossbeamThreadPipeEndpointA<ServerMarionetteCommand, ()>;

#[derive(Clone)]
pub struct MarionetteServerPlugin {
    pipe: CrossbeamThreadPipe<ServerMarionetteCommand, ()>,
}

impl MarionetteServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> ServerMarionetteIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for MarionetteServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MarionetteServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ServerPipeResource(self.pipe.endpoint_b()))
            .add_message::<Movement3DMessage>()
            .add_message::<RotationSyncMessage>()
            .add_message::<DestroyBlockControllerMessage>()
            .add_message::<PlaceBlockControllerMessage>()
            .add_message::<BlockInteractionMessage>()
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
                (apply_rotation_sync, accept_block_interactions)
                    .in_set(MarionetteServerSet::Controllers),
            )
            .add_systems(
                FixedUpdate,
                apply_movement.in_set(MarionetteServerSet::Movement),
            );
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum MarionetteServerSet {
    Commands,
    Controllers,
    Movement,
    BlockInteractions,
}

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkControllerTarget {
    pub connection_id: ConnectionId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ServerMarionetteCommand {
    UsePlayerController {
        connection_id: ConnectionId,
        command: PlayerControllerCommand,
    },
}

#[derive(Resource, Clone)]
struct ServerPipeResource(CrossbeamThreadPipeEndpointB<ServerMarionetteCommand, ()>);

fn process_controller_commands(
    pipe: Res<ServerPipeResource>,
    targets: Query<(Entity, &NetworkControllerTarget)>,
    movements: Query<&super::Movement3D>,
    mut movement_messages: MessageWriter<Movement3DMessage>,
    mut rotation_messages: MessageWriter<RotationSyncMessage>,
    mut destroy_block_messages: MessageWriter<DestroyBlockControllerMessage>,
    mut place_block_messages: MessageWriter<PlaceBlockControllerMessage>,
) {
    let targets = targets
        .iter()
        .map(|(entity, target)| (target.connection_id, entity))
        .collect::<HashMap<_, _>>();
    let mut movement_by_entity =
        HashMap::<Entity, roundo_networking::protocol::ControllerCommand<Movement3DAction>>::new();
    let mut rotation_by_entity = HashMap::new();

    for _ in 0..MAX_CONTROL_COMMANDS_PER_TICK {
        let Some(ServerMarionetteCommand::UsePlayerController {
            connection_id,
            command,
        }) = pipe.0.try_receive()
        else {
            break;
        };
        let Some(&entity) = targets.get(&connection_id) else {
            continue;
        };
        match command {
            PlayerControllerCommand::Movement3D(command) => {
                let accepted_movement_sequence = movements
                    .get(entity)
                    .map_or(0, super::Movement3D::last_accepted_sequence);
                if command.sequence <= accepted_movement_sequence {
                    continue;
                }
                movement_by_entity
                    .entry(entity)
                    .and_modify(
                        |pending: &mut roundo_networking::protocol::ControllerCommand<_>| {
                            if command.sequence > pending.sequence {
                                for axis in 0..3 {
                                    pending.action.translation_delta[axis] +=
                                        command.action.translation_delta[axis];
                                }
                                pending.sequence = command.sequence;
                            }
                        },
                    )
                    .or_insert(command);
            }
            PlayerControllerCommand::SyncRotation(sync) => {
                rotation_by_entity.insert(entity, sync);
            }
            PlayerControllerCommand::DestroyBlock(command) => {
                destroy_block_messages.write(DestroyBlockControllerMessage { entity, command });
            }
            PlayerControllerCommand::PlaceBlock(command) => {
                place_block_messages.write(PlaceBlockControllerMessage { entity, command });
            }
        }
    }

    for (entity, command) in movement_by_entity {
        movement_messages.write(Movement3DMessage { entity, command });
    }
    for (entity, sync) in rotation_by_entity {
        rotation_messages.write(RotationSyncMessage { entity, sync });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControllerCommand, Movement3D, Movement3DAction, PlayerControllers, RotationSync};
    use bevy::prelude::{App, Quat, Time, Transform, Vec3};

    #[test]
    fn movement_delta_is_added_without_using_rotation() {
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
                    translation_delta: [1.0, 2.0, 3.0],
                },
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0)
        );
    }

    #[test]
    fn rotation_sync_sets_transform_without_a_rotation_controller() {
        let plugin = MarionetteServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.init_resource::<Time<bevy::prelude::Fixed>>()
            .add_plugins(plugin);
        let expected = Quat::from_rotation_y(0.25);
        let entity = app
            .world_mut()
            .spawn((
                NetworkControllerTarget {
                    connection_id: ConnectionId(8),
                },
                Transform::default(),
            ))
            .id();

        ipc.try_send(ServerMarionetteCommand::UsePlayerController {
            connection_id: ConnectionId(8),
            command: PlayerControllerCommand::SyncRotation(RotationSync {
                rotation: expected.to_array(),
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);

        assert!(
            app.world()
                .get::<Transform>(entity)
                .unwrap()
                .rotation
                .abs_diff_eq(expected, f32::EPSILON)
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
                        translation_delta: [0.01, 0.0, 0.0],
                    },
                }),
            })
            .unwrap();
        }
        app.world_mut().run_schedule(FixedUpdate);

        assert!(
            app.world()
                .get::<Transform>(entity)
                .unwrap()
                .translation
                .abs_diff_eq(Vec3::X, 0.0001)
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
                        translation_delta: [1.0, 0.0, 0.0],
                    },
                }),
            })
            .unwrap();
        }
        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::X
        );
    }
}
