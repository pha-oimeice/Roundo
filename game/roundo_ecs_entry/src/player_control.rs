//! Server composition seam for Player identity and Controller wire operations.

use bevy::prelude::{App, FixedUpdate, IntoScheduleConfigs, Plugin, Resource, SystemSet, World};
use roundo_contracts::{
    ConnectionId, ControllerControlState, ControllerId, ControllerIntentDomain,
    ControllerOperationError, DirectedControllerInput, PlayerControllerAccess,
    PlayerControllerAccessSnapshot, PlayerControllerOperation, PlayerId,
};
use roundo_controller::{
    AcquireControlOutcome, ControllerControlError, ControllerInput, acquire_control,
    controller_intent_body, controller_state, release_control, submit_input,
};
use roundo_creature::{GazeIntent, MovementIntent};
use roundo_player::{ExternalIdentityKey, Player, PlayerRegistry, find_or_create_player};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{BTreeMap, HashMap};

pub type PlayerControlServerIpc =
    CrossbeamThreadPipeEndpointA<PlayerControlCommand, PlayerControlEvent>;

#[derive(Clone, Debug)]
pub enum PlayerControlCommand {
    Connected {
        connection_id: ConnectionId,
        external_identity: String,
    },
    Disconnected {
        connection_id: ConnectionId,
    },
    RequestAccess {
        connection_id: ConnectionId,
    },
    Acquire {
        connection_id: ConnectionId,
        controller_id: ControllerId,
    },
    Release {
        connection_id: ConnectionId,
        controller_id: ControllerId,
    },
    Submit {
        connection_id: ConnectionId,
        controller_id: ControllerId,
        input: DirectedControllerInput,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerControlEvent {
    Connected {
        connection_id: ConnectionId,
        player_id: PlayerId,
    },
    AccessSnapshot {
        connection_id: ConnectionId,
        snapshot: PlayerControllerAccessSnapshot,
    },
    Acquired {
        connection_id: ConnectionId,
        controller_id: ControllerId,
    },
    Released {
        connection_id: ConnectionId,
        controller_id: ControllerId,
    },
    Rejected {
        connection_id: ConnectionId,
        operation: PlayerControllerOperation,
        error: ControllerOperationError,
    },
}

#[derive(Clone)]
pub struct PlayerControlServerPlugin {
    pipe: CrossbeamThreadPipe<PlayerControlCommand, PlayerControlEvent>,
}

impl PlayerControlServerPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }
    pub fn ipc(&self) -> PlayerControlServerIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for PlayerControlServerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Resource)]
struct PlayerControlPipe(CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub(crate) enum PlayerControlServerSet {
    Commands,
}

/// Sequence state belongs to each runtime Controller rather than a connection.
/// Movement retains only the latest accepted sequence; gaze keeps its last
/// accepted sequence after all same-tick deltas have been accumulated.
#[derive(Resource, Default)]
pub(crate) struct ControllerInputSequences {
    movement: HashMap<ControllerId, u64>,
    gaze: HashMap<ControllerId, u64>,
}

impl ControllerInputSequences {
    pub(crate) fn movement_sequence(&self, controller_id: ControllerId) -> u64 {
        self.movement.get(&controller_id).copied().unwrap_or(0)
    }

    pub(crate) fn gaze_sequence(&self, controller_id: ControllerId) -> u64 {
        self.gaze.get(&controller_id).copied().unwrap_or(0)
    }
}

impl Plugin for PlayerControlServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PlayerControlPipe(self.pipe.endpoint_b()))
            .init_resource::<ControllerInputSequences>()
            .add_systems(
                FixedUpdate,
                process_player_control.in_set(PlayerControlServerSet::Commands),
            );
    }
}

fn process_player_control(world: &mut World) {
    let pipe = world.resource::<PlayerControlPipe>().0.clone();
    let mut movement = HashMap::<ControllerId, (bevy::prelude::Entity, u64, [f32; 3])>::new();
    let mut gaze =
        BTreeMap::<ControllerId, (bevy::prelude::Entity, BTreeMap<u64, (f32, f32)>)>::new();
    while let Some(command) = pipe.try_receive() {
        match command {
            PlayerControlCommand::Connected {
                connection_id,
                external_identity,
            } => {
                let Ok(identity) = ExternalIdentityKey::new(external_identity) else {
                    continue;
                };
                let player = find_or_create_player(world, identity);
                crate::authoritative_creature::ensure_test_life_for_player(world, player);
                if world
                    .resource_mut::<PlayerRegistry>()
                    .bind_connection(connection_id, player)
                    .is_ok()
                {
                    let _ = pipe.try_send(PlayerControlEvent::Connected {
                        connection_id,
                        player_id: player,
                    });
                }
            }
            PlayerControlCommand::Disconnected { connection_id } => {
                world.init_resource::<PlayerRegistry>();
                world
                    .resource_mut::<PlayerRegistry>()
                    .unbind_connection(connection_id);
            }
            PlayerControlCommand::RequestAccess { connection_id } => {
                let Some(player_id) = player_for_connection(world, connection_id) else {
                    let _ = pipe.try_send(PlayerControlEvent::Rejected {
                        connection_id,
                        operation: PlayerControllerOperation::RequestAccess,
                        error: ControllerOperationError::AccessDenied,
                    });
                    continue;
                };
                let Some(player_entity) =
                    world.resource::<PlayerRegistry>().player_entity(player_id)
                else {
                    continue;
                };
                let Some(player) = world.get::<Player>(player_entity) else {
                    continue;
                };
                let mut controllers = player
                    .controller_accesses()
                    .filter_map(|(controller_id, access_level)| {
                        let (domain, current) = controller_state(world, controller_id)?;
                        let intent_domain = if domain == roundo_controller::IntentDomain::movement()
                        {
                            ControllerIntentDomain::Movement
                        } else if domain == roundo_controller::IntentDomain::gaze() {
                            ControllerIntentDomain::Gaze
                        } else {
                            return None;
                        };
                        Some(PlayerControllerAccess {
                            controller_id,
                            intent_domain,
                            access_level,
                            control_state: match current {
                                None => ControllerControlState::Uncontrolled,
                                Some(id) if id == player_id => {
                                    ControllerControlState::ControlledBySelf
                                }
                                Some(_) => ControllerControlState::ControlledByOther,
                            },
                        })
                    })
                    .collect::<Vec<_>>();
                controllers.sort_unstable_by_key(|entry| entry.controller_id);
                let _ = pipe.try_send(PlayerControlEvent::AccessSnapshot {
                    connection_id,
                    snapshot: PlayerControllerAccessSnapshot {
                        player_id,
                        controllers,
                    },
                });
            }
            PlayerControlCommand::Acquire {
                connection_id,
                controller_id,
            } => {
                let result = player_for_connection(world, connection_id)
                    .ok_or(ControllerOperationError::AccessDenied)
                    .and_then(|player| {
                        acquire_control(world, player, controller_id)
                            .map(|outcome| (player, outcome))
                            .map_err(map_error)
                    });
                match result {
                    Ok((
                        _,
                        AcquireControlOutcome::Acquired | AcquireControlOutcome::AlreadyControlled,
                    )) => {
                        let _ = pipe.try_send(PlayerControlEvent::Acquired {
                            connection_id,
                            controller_id,
                        });
                    }
                    Err(error) => {
                        let _ = pipe.try_send(PlayerControlEvent::Rejected {
                            connection_id,
                            operation: PlayerControllerOperation::Acquire { controller_id },
                            error,
                        });
                    }
                }
            }
            PlayerControlCommand::Release {
                connection_id,
                controller_id,
            } => {
                let result = player_for_connection(world, connection_id)
                    .ok_or(ControllerOperationError::AccessDenied)
                    .and_then(|player| {
                        release_control(world, player, controller_id).map_err(map_error)
                    });
                match result {
                    Ok(()) => {
                        let _ = pipe.try_send(PlayerControlEvent::Released {
                            connection_id,
                            controller_id,
                        });
                    }
                    Err(error) => {
                        let _ = pipe.try_send(PlayerControlEvent::Rejected {
                            connection_id,
                            operation: PlayerControllerOperation::Release { controller_id },
                            error,
                        });
                    }
                }
            }
            PlayerControlCommand::Submit {
                connection_id,
                controller_id,
                input,
            } => {
                let Some(player) = player_for_connection(world, connection_id) else {
                    let _ = pipe.try_send(PlayerControlEvent::Rejected {
                        connection_id,
                        operation: PlayerControllerOperation::SubmitInput { controller_id },
                        error: ControllerOperationError::AccessDenied,
                    });
                    continue;
                };
                let (sequence, input) = match input {
                    DirectedControllerInput::Movement3D(command) => (
                        command.sequence,
                        ControllerInput::Movement {
                            local_axis: command.action.direction,
                        },
                    ),
                    DirectedControllerInput::Gaze(command) => (
                        command.sequence,
                        ControllerInput::Gaze {
                            yaw_delta: command.action.yaw_delta,
                            pitch_delta: command.action.pitch_delta,
                        },
                    ),
                };
                if let Err(error) = submit_input(world, player, controller_id, input) {
                    let _ = pipe.try_send(PlayerControlEvent::Rejected {
                        connection_id,
                        operation: PlayerControllerOperation::SubmitInput { controller_id },
                        error: map_error(error),
                    });
                    continue;
                }
                let Some(intent_body) = controller_intent_body(world, controller_id) else {
                    let _ = pipe.try_send(PlayerControlEvent::Rejected {
                        connection_id,
                        operation: PlayerControllerOperation::SubmitInput { controller_id },
                        error: ControllerOperationError::ControllerNotFound,
                    });
                    continue;
                };
                let sequences = world.resource::<ControllerInputSequences>();
                match input {
                    ControllerInput::Movement { local_axis }
                        if sequence
                            > sequences.movement.get(&controller_id).copied().unwrap_or(0) =>
                    {
                        movement
                            .entry(controller_id)
                            .and_modify(|pending| {
                                if sequence > pending.1 {
                                    *pending = (intent_body, sequence, local_axis);
                                }
                            })
                            .or_insert((intent_body, sequence, local_axis));
                    }
                    ControllerInput::Gaze {
                        yaw_delta,
                        pitch_delta,
                    } if sequence > sequences.gaze.get(&controller_id).copied().unwrap_or(0)
                        && yaw_delta.is_finite()
                        && pitch_delta.is_finite() =>
                    {
                        gaze.entry(controller_id)
                            .or_insert_with(|| (intent_body, BTreeMap::new()))
                            .1
                            .insert(sequence, (yaw_delta, pitch_delta));
                    }
                    _ => {}
                }
            }
        }
    }
    let mut accepted_movement = Vec::new();
    for (controller_id, (intent_body, sequence, local_axis)) in movement {
        if let Some(mut intent) = world.get_mut::<MovementIntent>(intent_body)
            && let Ok(value) = MovementIntent::new(local_axis)
        {
            *intent = value;
            accepted_movement.push((controller_id, sequence));
        }
    }
    let mut accepted_gaze = Vec::new();
    for (controller_id, (intent_body, deltas)) in gaze {
        let Some((&sequence, _)) = deltas.last_key_value() else {
            continue;
        };
        let (yaw_delta, pitch_delta) = deltas
            .values()
            .fold((0.0, 0.0), |sum, delta| (sum.0 + delta.0, sum.1 + delta.1));
        if let Some(mut intent) = world.get_mut::<GazeIntent>(intent_body)
            && let Ok(value) = GazeIntent::new(yaw_delta, pitch_delta)
        {
            *intent = value;
            accepted_gaze.push((controller_id, sequence));
        }
    }
    let mut sequences = world.resource_mut::<ControllerInputSequences>();
    sequences.movement.extend(accepted_movement);
    sequences.gaze.extend(accepted_gaze);
}

fn player_for_connection(world: &World, connection_id: ConnectionId) -> Option<PlayerId> {
    world
        .get_resource::<PlayerRegistry>()?
        .player_for_connection(connection_id)
}

fn map_error(error: ControllerControlError) -> ControllerOperationError {
    match error {
        ControllerControlError::ControllerNotFound => ControllerOperationError::ControllerNotFound,
        ControllerControlError::PlayerNotFound | ControllerControlError::AccessDenied { .. } => {
            ControllerOperationError::AccessDenied
        }
        ControllerControlError::ControlledByOther { .. } => {
            ControllerOperationError::ControlledByOther
        }
        ControllerControlError::NotCurrentController { .. } => {
            ControllerOperationError::NotCurrentController
        }
        ControllerControlError::InputOutsideDomain { .. } => {
            ControllerOperationError::InputOutsideDomain
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::App;
    use roundo_contracts::{
        ControllerAccessLevel, ControllerCommand, Movement3DAction, PlayerControllerOperation,
    };
    use roundo_controller::{IntentDomain, register_controller};
    use roundo_creature::{BodyInstanceIds, IntentBodyBundle, IntentBodyId, LifeRegistry};

    fn init_test_life_resources(app: &mut App) {
        app.init_resource::<BodyInstanceIds>()
            .init_resource::<LifeRegistry>();
    }

    fn connect(app: &mut App, ipc: &PlayerControlServerIpc, connection: u64, identity: &str) {
        ipc.try_send(PlayerControlCommand::Connected {
            connection_id: ConnectionId(connection),
            external_identity: identity.into(),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert!(matches!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Connected {
                connection_id: observed,
                ..
            }) if observed == ConnectionId(connection)
        ));
    }

    #[test]
    fn access_snapshot_is_limited_to_the_connection_player() {
        let plugin = PlayerControlServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.add_plugins(plugin);
        init_test_life_resources(&mut app);
        let intent = app
            .world_mut()
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let controller =
            register_controller(app.world_mut(), intent, IntentDomain::movement()).unwrap();
        connect(&mut app, &ipc, 1, "peer:a");
        connect(&mut app, &ipc, 2, "peer:b");
        let first = app
            .world()
            .resource::<PlayerRegistry>()
            .player_for_connection(ConnectionId(1))
            .unwrap();
        let first_entity = app
            .world()
            .resource::<PlayerRegistry>()
            .player_entity(first)
            .unwrap();
        app.world_mut()
            .get_mut::<Player>(first_entity)
            .unwrap()
            .set_controller_access(controller, ControllerAccessLevel::Read);
        ipc.try_send(PlayerControlCommand::RequestAccess {
            connection_id: ConnectionId(2),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        let PlayerControlEvent::AccessSnapshot { snapshot, .. } = ipc.try_receive().unwrap() else {
            panic!("expected snapshot")
        };
        assert!(
            snapshot
                .controllers
                .iter()
                .all(|entry| entry.controller_id != controller)
        );
        ipc.try_send(PlayerControlCommand::RequestAccess {
            connection_id: ConnectionId(1),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        let PlayerControlEvent::AccessSnapshot { snapshot, .. } = ipc.try_receive().unwrap() else {
            panic!("expected snapshot")
        };
        assert_eq!(snapshot.player_id, first);
        assert!(
            snapshot
                .controllers
                .iter()
                .any(|entry| entry.controller_id == controller)
        );
    }

    #[test]
    fn control_results_echo_the_operation_without_exposing_another_player() {
        let plugin = PlayerControlServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.add_plugins(plugin);
        init_test_life_resources(&mut app);
        let intent = app
            .world_mut()
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let controller =
            register_controller(app.world_mut(), intent, IntentDomain::movement()).unwrap();
        connect(&mut app, &ipc, 1, "peer:a");
        connect(&mut app, &ipc, 2, "peer:b");

        for connection in [ConnectionId(1), ConnectionId(2)] {
            let player_id = app
                .world()
                .resource::<PlayerRegistry>()
                .player_for_connection(connection)
                .unwrap();
            let player_entity = app
                .world()
                .resource::<PlayerRegistry>()
                .player_entity(player_id)
                .unwrap();
            app.world_mut()
                .get_mut::<Player>(player_entity)
                .unwrap()
                .set_controller_access(controller, ControllerAccessLevel::ReadWrite);
        }

        ipc.try_send(PlayerControlCommand::Acquire {
            connection_id: ConnectionId(1),
            controller_id: controller,
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Acquired {
                connection_id: ConnectionId(1),
                controller_id: controller,
            })
        );

        ipc.try_send(PlayerControlCommand::Acquire {
            connection_id: ConnectionId(2),
            controller_id: controller,
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Rejected {
                connection_id: ConnectionId(2),
                operation: PlayerControllerOperation::Acquire {
                    controller_id: controller,
                },
                error: ControllerOperationError::ControlledByOther,
            })
        );

        ipc.try_send(PlayerControlCommand::Release {
            connection_id: ConnectionId(1),
            controller_id: controller,
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Released {
                connection_id: ConnectionId(1),
                controller_id: controller,
            })
        );

        ipc.try_send(PlayerControlCommand::Submit {
            connection_id: ConnectionId(1),
            controller_id: controller,
            input: DirectedControllerInput::Movement3D(ControllerCommand {
                sequence: 1,
                action: Movement3DAction {
                    direction: [1.0, 0.0, 0.0],
                },
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Rejected {
                connection_id: ConnectionId(1),
                operation: PlayerControllerOperation::SubmitInput {
                    controller_id: controller,
                },
                error: ControllerOperationError::NotCurrentController,
            })
        );
    }

    #[test]
    fn directed_inputs_update_only_their_orthogonal_intents_with_sequence_rules() {
        let plugin = PlayerControlServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.add_plugins(plugin);
        init_test_life_resources(&mut app);
        let intent = app
            .world_mut()
            .spawn(IntentBodyBundle::materialize(IntentBodyId(1)))
            .id();
        let movement =
            register_controller(app.world_mut(), intent, IntentDomain::movement()).unwrap();
        let gaze = register_controller(app.world_mut(), intent, IntentDomain::gaze()).unwrap();
        connect(&mut app, &ipc, 1, "peer:a");
        let player_id = app
            .world()
            .resource::<PlayerRegistry>()
            .player_for_connection(ConnectionId(1))
            .unwrap();
        let player_entity = app
            .world()
            .resource::<PlayerRegistry>()
            .player_entity(player_id)
            .unwrap();
        let mut player = app.world_mut().get_mut::<Player>(player_entity).unwrap();
        player.set_controller_access(movement, ControllerAccessLevel::ReadWrite);
        player.set_controller_access(gaze, ControllerAccessLevel::ReadWrite);
        drop(player);
        for controller_id in [movement, gaze] {
            ipc.try_send(PlayerControlCommand::Acquire {
                connection_id: ConnectionId(1),
                controller_id,
            })
            .unwrap();
        }
        app.world_mut().run_schedule(FixedUpdate);
        while ipc.try_receive().is_some() {}

        for (sequence, direction) in [(1, [1.0, 0.0, 0.0]), (2, [0.0, 1.0, 0.0])] {
            ipc.try_send(PlayerControlCommand::Submit {
                connection_id: ConnectionId(1),
                controller_id: movement,
                input: DirectedControllerInput::Movement3D(ControllerCommand {
                    sequence,
                    action: Movement3DAction { direction },
                }),
            })
            .unwrap();
        }
        for (sequence, yaw_delta) in [(1, 0.25), (2, 0.5)] {
            ipc.try_send(PlayerControlCommand::Submit {
                connection_id: ConnectionId(1),
                controller_id: gaze,
                input: DirectedControllerInput::Gaze(ControllerCommand {
                    sequence,
                    action: roundo_contracts::GazeIntent {
                        yaw_delta,
                        pitch_delta: 0.0,
                    },
                }),
            })
            .unwrap();
        }
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            app.world()
                .get::<MovementIntent>(intent)
                .unwrap()
                .local_axis,
            [0.0, 1.0, 0.0]
        );
        assert_eq!(
            app.world().get::<GazeIntent>(intent).unwrap().yaw_delta,
            0.75
        );

        ipc.try_send(PlayerControlCommand::Submit {
            connection_id: ConnectionId(1),
            controller_id: movement,
            input: DirectedControllerInput::Movement3D(ControllerCommand {
                sequence: 1,
                action: Movement3DAction {
                    direction: [-1.0, 0.0, 0.0],
                },
            }),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            app.world()
                .get::<MovementIntent>(intent)
                .unwrap()
                .local_axis,
            [0.0, 1.0, 0.0]
        );
    }

    #[test]
    fn same_tick_connection_and_access_request_observe_bootstrap_grants() {
        let plugin = PlayerControlServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.add_plugins(plugin);
        init_test_life_resources(&mut app);
        ipc.try_send(PlayerControlCommand::Connected {
            connection_id: ConnectionId(1),
            external_identity: "peer:a".into(),
        })
        .unwrap();
        ipc.try_send(PlayerControlCommand::RequestAccess {
            connection_id: ConnectionId(1),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert!(matches!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Connected {
                connection_id: ConnectionId(1),
                ..
            })
        ));
        let Some(PlayerControlEvent::AccessSnapshot { snapshot, .. }) = ipc.try_receive() else {
            panic!("expected access snapshot")
        };
        assert_eq!(snapshot.controllers.len(), 2);
        assert!(snapshot.controllers.iter().all(|entry| {
            entry.access_level == ControllerAccessLevel::ReadWrite
                && entry.control_state == ControllerControlState::Uncontrolled
        }));
    }

    #[test]
    fn unbound_access_request_returns_an_explicit_rejection() {
        let plugin = PlayerControlServerPlugin::new();
        let ipc = plugin.ipc();
        let mut app = App::new();
        app.add_plugins(plugin);
        ipc.try_send(PlayerControlCommand::RequestAccess {
            connection_id: ConnectionId(99),
        })
        .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            ipc.try_receive(),
            Some(PlayerControlEvent::Rejected {
                connection_id: ConnectionId(99),
                operation: PlayerControllerOperation::RequestAccess,
                error: ControllerOperationError::AccessDenied,
            })
        );
    }
}
