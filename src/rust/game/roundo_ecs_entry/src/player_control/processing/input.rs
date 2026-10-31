//! Owns tick-local input aggregation and persistent Controller sequence state.

use super::*;

/// Sequence state belongs to each runtime Controller rather than a connection.
/// Movement retains only the latest accepted sequence; gaze keeps its last
/// accepted sequence after all same-tick deltas have been accumulated.
#[derive(Resource, Default)]
pub(crate) struct ControllerInputSequences {
    movement: HashMap<ControllerId, u64>,
    gaze: HashMap<ControllerId, u64>,
    interaction: HashMap<ControllerId, u64>,
}

impl ControllerInputSequences {
    pub(crate) fn movement_sequence(&self, controller_id: ControllerId) -> u64 {
        self.movement.get(&controller_id).copied().unwrap_or(0)
    }

    pub(crate) fn gaze_sequence(&self, controller_id: ControllerId) -> u64 {
        self.gaze.get(&controller_id).copied().unwrap_or(0)
    }

    pub(in crate::player_control) fn interaction_sequence(
        &self,
        controller_id: ControllerId,
    ) -> u64 {
        self.interaction.get(&controller_id).copied().unwrap_or(0)
    }

    pub(in crate::player_control) fn forget_controller(&mut self, controller_id: ControllerId) {
        self.movement.remove(&controller_id);
        self.gaze.remove(&controller_id);
        self.interaction.remove(&controller_id);
    }
}

#[derive(Default)]
pub(super) struct ControllerTickInputs {
    movement: HashMap<ControllerId, (u64, [f32; 3])>,
    gaze: BTreeMap<ControllerId, BTreeMap<u64, (f32, f32)>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DiscardedInputSequences {
    pub(super) movement: Option<u64>,
    pub(super) gaze: Option<u64>,
}

impl ControllerTickInputs {
    pub(super) fn queue(
        &mut self,
        world: &mut World,
        controller_id: ControllerId,
        input: DirectedControllerInput,
    ) -> bool {
        let Some(route) = resolve_controller_command_route(world, controller_id) else {
            return false;
        };
        match input {
            DirectedControllerInput::SetGaze(command)
                if route.domain.contains(roundo_controller::IntentKind::Gaze) =>
            {
                if command.sequence
                    <= world
                        .resource::<ControllerInputSequences>()
                        .gaze_sequence(controller_id)
                    || self.gaze.contains_key(&controller_id)
                {
                    return false;
                }
                let published = world
                    .run_system_once_with(
                        commands::publish_set_gaze,
                        (controller_id, command.action.orientation),
                    )
                    .expect("set gaze uses native registered Intent writer");
                if published {
                    world
                        .resource_mut::<ControllerInputSequences>()
                        .gaze
                        .insert(controller_id, command.sequence);
                }
                published
            }
            DirectedControllerInput::UseItem(command)
                if route
                    .domain
                    .contains(roundo_controller::IntentKind::BlockInteraction) =>
            {
                if command.sequence
                    <= world
                        .resource::<ControllerInputSequences>()
                        .interaction_sequence(controller_id)
                {
                    return false;
                }
                let use_item = roundo_block_interaction::ItemUseCommandEvent {
                    item: roundo_inventory::ItemId::from_u128(command.action.item.0),
                    method: command.action.method,
                };
                let published = world
                    .run_system_once_with(commands::publish_interaction, (controller_id, use_item))
                    .expect("block Command uses native registered Intent writer");
                if published {
                    world
                        .resource_mut::<ControllerInputSequences>()
                        .interaction
                        .insert(controller_id, command.sequence);
                    world.run_system_once_with(crate::authoritative_creature::player_interaction::trigger,
                        FactoryMotionRoute::new(route.life, route.intent, route.execution,
                            world.resource::<crate::authoritative_creature::CreatureAssemblyRegistry>().get(route.life).unwrap().record))
                        .expect("Execution triggers through native read registrations");
                }
                published
            }
            DirectedControllerInput::Movement3D(command)
                if route
                    .domain
                    .contains(roundo_controller::IntentKind::Movement) =>
            {
                let sequence = command.sequence;
                if sequence
                    <= world
                        .resource::<ControllerInputSequences>()
                        .movement_sequence(controller_id)
                    || sequence <= self.movement.get(&controller_id).map_or(0, |(seq, _)| *seq)
                    || MovementCommandState::new(command.action.direction).is_err()
                {
                    return false;
                }
                self.movement
                    .insert(controller_id, (sequence, command.action.direction));
                true
            }
            DirectedControllerInput::Gaze(command)
                if route.domain.contains(roundo_controller::IntentKind::Gaze) =>
            {
                let sequence = command.sequence;
                if sequence
                    <= world
                        .resource::<ControllerInputSequences>()
                        .gaze_sequence(controller_id)
                    || self
                        .gaze
                        .get(&controller_id)
                        .is_some_and(|deltas| deltas.contains_key(&sequence))
                    || GazeCommandEvent::new(command.action.yaw_delta, command.action.pitch_delta)
                        .is_err()
                {
                    return false;
                }
                self.gaze.entry(controller_id).or_default().insert(
                    sequence,
                    (command.action.yaw_delta, command.action.pitch_delta),
                );
                true
            }
            _ => false,
        }
    }

    pub(super) fn discard(&mut self, controller_id: ControllerId) {
        self.take_pending(controller_id);
    }

    pub(super) fn begin_binding(world: &mut World, controller_id: ControllerId) {
        world
            .resource_mut::<ControllerInputSequences>()
            .forget_controller(controller_id);
    }

    pub(super) fn discard_ai_prefix(
        &mut self,
        world: &mut World,
        controller_id: ControllerId,
        valid_old_binding: bool,
    ) -> DiscardedInputSequences {
        let discarded = self.take_pending(controller_id);
        if valid_old_binding {
            let mut sequences = world.resource_mut::<ControllerInputSequences>();
            if let Some(sequence) = discarded.movement {
                sequences.movement.insert(controller_id, sequence);
            }
            if let Some(sequence) = discarded.gaze {
                sequences.gaze.insert(controller_id, sequence);
            }
        }
        discarded
    }

    fn take_pending(&mut self, controller_id: ControllerId) -> DiscardedInputSequences {
        DiscardedInputSequences {
            movement: self
                .movement
                .remove(&controller_id)
                .map(|(sequence, _)| sequence),
            gaze: self
                .gaze
                .remove(&controller_id)
                .and_then(|deltas| deltas.last_key_value().map(|(&sequence, _)| sequence)),
        }
    }

    pub(super) fn publish(self, world: &mut World) {
        let mut accepted_movement = Vec::new();
        for (controller_id, (sequence, local_axis)) in self.movement {
            if publish_movement_command(world, controller_id, local_axis) {
                accepted_movement.push((controller_id, sequence));
            }
        }
        let mut accepted_gaze = Vec::new();
        for (controller_id, deltas) in self.gaze {
            let Some((&sequence, _)) = deltas.last_key_value() else {
                continue;
            };
            let (yaw_delta, pitch_delta) = deltas
                .values()
                .fold((0.0, 0.0), |sum, delta| (sum.0 + delta.0, sum.1 + delta.1));
            if publish_gaze_command(world, controller_id, yaw_delta, pitch_delta) {
                accepted_gaze.push((controller_id, sequence));
            }
        }
        let mut sequences = world.resource_mut::<ControllerInputSequences>();
        sequences.movement.extend(accepted_movement);
        sequences.gaze.extend(accepted_gaze);
    }
}

fn publish_movement_command(
    world: &mut World,
    controller_id: ControllerId,
    local_axis: [f32; 3],
) -> bool {
    world
        .run_system_once_with(commands::publish_movement, (controller_id, local_axis))
        .expect("Command publication has its declared Life data resources")
}

fn publish_gaze_command(
    world: &mut World,
    controller_id: ControllerId,
    yaw_delta: f32,
    pitch_delta: f32,
) -> bool {
    world
        .run_system_once_with(
            commands::publish_gaze,
            (controller_id, yaw_delta, pitch_delta),
        )
        .expect("Command publication has its declared Life data resources")
}
