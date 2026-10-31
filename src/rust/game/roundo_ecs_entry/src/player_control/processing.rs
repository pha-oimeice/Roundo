//! Live factory routing, command publication and domain-scoped neutralization.

use super::*;
use bevy::ecs::system::RunSystemOnce;
mod commands;
mod dispatch;
mod input;
pub(crate) use input::ControllerInputSequences;
use input::ControllerTickInputs;
mod inventory;
mod reads;
pub(super) use dispatch::process_player_control;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ControllerCommandRoute {
    life: LifeHandle,
    intent: ConceptBodyHandle,
    execution: ConceptBodyHandle,
    domain: roundo_controller::IntentDomain,
}

/// Existing FIFO bridge only; the read-only system owns all routing rules.
/// Mutable World is needed to initialize/run the system, not to mutate facts.
fn resolve_controller_command_route(
    world: &mut World,
    controller_id: ControllerId,
) -> Option<ControllerCommandRoute> {
    world
        .run_system_once_with(reads::resolve_route, controller_id)
        .expect("route probe has its declared read-only parameters")
}

/// Factory Controllers route only through a live assembly and its live Life.
/// Entity-only Controllers are explicit compatibility registrations.
pub(crate) fn controller_intent_body(
    world: &mut World,
    id: ControllerId,
) -> Option<bevy::prelude::Entity> {
    world
        .run_system_once_with(reads::resolve_intent_entity, id)
        .expect("Intent probe has its declared read-only parameters")
}

pub(super) fn neutralize_controller(world: &mut World, id: ControllerId) -> bool {
    world
        .run_system_once_with(commands::neutralize, id)
        .expect("Command neutralization has its declared Life data resources")
}

fn neutralize_gate_player(world: &mut World, player: PlayerId) -> Vec<ControllerId> {
    world
        .run_system_once_with(commands::neutralize_player, player)
        .expect("Player neutralization has its declared admission and Life data resources")
}

fn prefix_player(
    world: &mut World,
    connection_id: ConnectionId,
    controller_id: ControllerId,
) -> Option<PlayerId> {
    world
        .run_system_once_with(reads::resolve_prefix_player, (connection_id, controller_id))
        .expect("prefix probe has its declared read-only parameters")
}

fn player_for_connection(world: &World, connection_id: ConnectionId) -> Option<PlayerId> {
    world
        .get_resource::<PlayerRegistry>()?
        .player_for_connection(connection_id)
}
