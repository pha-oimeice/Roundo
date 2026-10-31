//! Owns session binding, authority invalidation and last-connection teardown.

use super::*;

/// Reconnect invalidates the connection and the Player; disconnect only the
/// connection. Keep that selection identical across the independent proof books.
struct InvalidatedSession {
    connection_id: ConnectionId,
    player_id: Option<PlayerId>,
}

impl InvalidatedSession {
    fn matches(&self, connection_id: ConnectionId, player_id: PlayerId) -> bool {
        self.connection_id == connection_id || self.player_id == Some(player_id)
    }

    fn fail_tickets(
        &self,
        world: &mut World,
        pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
        inputs: &mut ControllerTickInputs,
    ) {
        for id in fail_ticketed_where(world, pipe, |entry| {
            self.matches(entry.connection_id, entry.player_id)
        }) {
            inputs.discard(id);
        }
    }

    fn discard_proofs(&self, world: &mut World) {
        world
            .resource_mut::<PrefixRequests>()
            .remove_where(|entry| self.matches(entry.connection_id, entry.player_id));
        world
            .resource_mut::<FreezeDispatchResults>()
            .fail_where(|entry| self.matches(entry.connection_id, entry.player_id));
        world
            .resource_mut::<SettlementCertificates>()
            .discard_where(|certificate| {
                self.matches(certificate.connection.id, certificate.player_id)
            });
    }
}

pub(super) fn connected(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    inputs: &mut ControllerTickInputs,
    connection_id: ConnectionId,
    external_identity: String,
) {
    let Ok(identity) = ExternalIdentityKey::new(external_identity) else {
        return;
    };
    let player = find_or_create_player(world, identity);
    crate::authoritative_creature::ensure_test_life_for_player(world, player);
    if world
        .resource_mut::<PlayerRegistry>()
        .bind_connection(connection_id, player)
        .is_err()
    {
        return;
    }
    let invalidated = InvalidatedSession {
        connection_id,
        player_id: Some(player),
    };
    invalidated.fail_tickets(world, pipe, inputs);
    // Connected historically neutralizes without discarding this returned list.
    // Do not widen that path to the disconnect-only pending-input cleanup.
    neutralize_gate_player(world, player);
    world
        .resource_mut::<Admission>()
        .revoke_connection(connection_id);
    world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
        let _ = admission.authenticated_connected(world, connection_id);
    });
    let mut requests = world.resource_mut::<PrefixRequests>();
    requests.next_generation = requests
        .next_generation
        .checked_add(1)
        .expect("session generation exhausted");
    let generation = requests.next_generation;
    requests.generations.insert(connection_id, generation);
    drop(requests);
    // In particular, Connected preserves both replay high-water books.
    invalidated.discard_proofs(world);
    let _ = pipe.try_send(PlayerControlEvent::Connected {
        connection_id,
        player_id: player,
    });
}

pub(super) fn disconnected(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    inputs: &mut ControllerTickInputs,
    connection_id: ConnectionId,
) {
    if let Some(mut ingress) = world.get_resource_mut::<crate::debug::DebugIngress>() {
        ingress.forget(connection_id);
    }
    let invalidated = InvalidatedSession {
        connection_id,
        player_id: None,
    };
    invalidated.fail_tickets(world, pipe, inputs);
    if let Some(player) = player_for_connection(world, connection_id) {
        for id in neutralize_gate_player(world, player) {
            inputs.discard(id);
        }
    }
    world
        .resource_mut::<Admission>()
        .revoke_connection(connection_id);
    world
        .resource_mut::<TicketedResults>()
        .highest_ids
        .remove(&connection_id);
    world
        .resource_mut::<PrefixRequests>()
        .generations
        .remove(&connection_id);
    invalidated.discard_proofs(world);
    world.init_resource::<PlayerRegistry>();
    let disconnected_player = world
        .resource_mut::<PlayerRegistry>()
        .unbind_connection(connection_id);
    if let Some(player_id) = disconnected_player {
        let has_connection = world
            .resource::<PlayerRegistry>()
            .connections_for_player(player_id)
            .next()
            .is_some();
        if !has_connection {
            let revoked = world
                .query::<&roundo_controller::Controller>()
                .iter(world)
                .filter(|controller| {
                    controller.current_owner() == Some(ControllerOwner::Player(player_id))
                })
                .map(|controller| controller.id())
                .collect::<Vec<_>>();
            release_all_control(world, player_id);
            for id in revoked {
                inputs.discard(id);
            }
        }
    }
}
