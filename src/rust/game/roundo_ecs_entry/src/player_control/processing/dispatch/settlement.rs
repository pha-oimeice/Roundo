//! Mint one-shot settlement evidence only after accepted input is published.

use super::*;

pub(super) fn settle_pending_prefixes(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    now: u64,
    pending_acks: Vec<(ControlRequestId, PrefixRequest)>,
) {
    // Settle the old binding only after the entire FIFO tick has applied its
    // accepted input and sequence high-water marks. Never reset the old sequence
    // on failed drain; Acquire establishes a fresh namespace after success.
    for (request_id, entry) in pending_acks {
        if let Some(ticket) = world
            .resource::<TicketedResults>()
            .pending
            .get(&(entry.connection_id, request_id))
            .copied()
            .filter(|ticket| ticket.kind == ControlRequestKind::Release)
        {
            let tick = now;
            let valid = tick < ticket.deadline
                && ticket.generation == entry.generation
                && ticket.player_id == entry.player_id
                && ticket.controller_id == entry.controller_id
                && world
                    .resource::<PrefixRequests>()
                    .generations
                    .get(&entry.connection_id)
                    == Some(&entry.generation)
                && player_for_connection(world, entry.connection_id) == Some(entry.player_id)
                && world
                    .resource::<PlayerRegistry>()
                    .connections_for_player(entry.player_id)
                    .count()
                    == 1
                && world
                    .resource::<PlayerRegistry>()
                    .player_entity(entry.player_id)
                    .and_then(|entity| world.get::<Player>(entity))
                    .is_some_and(|player| {
                        player.controller_access(entry.controller_id)
                            == roundo_contracts::ControllerAccessLevel::ReadWrite
                            && player.controller_access_revision(entry.controller_id)
                                == Some(ticket.access_revision)
                    })
                && world
                    .resource::<FreezeDispatchResults>()
                    .pending
                    .get(&(entry.connection_id, request_id))
                    .is_some_and(|pending| pending.queued && pending.reported && !pending.observed);
            if !valid {
                continue;
            }
            let Some(old_binding) = ticket.old_binding else {
                continue;
            };
            // A gate in Draining no longer exposes submit or an active binding;
            // this exact request and old binding were captured before closing it.
            if world
                .resource::<Admission>()
                .state(world, entry.controller_id)
                != roundo_controller::authority_gate::GateState::Draining
            {
                continue;
            }
            if !neutralize_controller(world, entry.controller_id) {
                continue;
            }
            let certificate = SettlementCertificate {
                connection: ticket.connection,
                player_id: ticket.player_id,
                controller_id: ticket.controller_id,
                request_id: ticket.gate_request,
                old_binding,
            };
            world
                .resource_mut::<SettlementCertificates>()
                .issued
                .insert(ControlRequestId(ticket.gate_request.0), certificate);
            let certificate = world.resource_mut::<SettlementCertificates>().try_consume(
                ticket.connection,
                ticket.player_id,
                ticket.controller_id,
                ticket.gate_request,
                old_binding,
            );
            let committed = certificate.is_some_and(|certificate| {
                world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
                    admission
                        .record_player_prefix(
                            world,
                            ticket.controller_id,
                            ticket.connection,
                            ticket.gate_request,
                            tick,
                        )
                        .and_then(|()| {
                            admission.commit_player_release(
                                world,
                                ticket.controller_id,
                                ticket.connection,
                                ticket.gate_request,
                                tick,
                                certificate,
                            )
                        })
                        .is_ok()
                })
            });
            world
                .resource_mut::<TicketedResults>()
                .pending
                .remove(&(entry.connection_id, request_id));
            world
                .resource_mut::<FreezeDispatchResults>()
                .discard((entry.connection_id, request_id));
            world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
                if !committed {
                    let _ =
                        admission.fail_transition(world, ticket.controller_id, ticket.gate_request);
                }
                admission.take_terminal(world, ticket.controller_id);
            });
            if committed {
                let _ = pipe.try_send(PlayerControlEvent::TicketedTerminal {
                    connection_id: entry.connection_id,
                    terminal: ControlTerminal {
                        request_id,
                        controller_id: entry.controller_id,
                        outcome: Ok(ControlSuccess::Released),
                    },
                });
            } else {
                ticketed_terminal(
                    &pipe,
                    entry.connection_id,
                    request_id,
                    entry.controller_id,
                    ControlHandoffError::NotReady,
                );
            }
            continue;
        }
        let requests = world.resource::<PrefixRequests>();
        if now >= entry.deadline
            || !requests
                .entries
                .get(&(entry.connection_id, request_id))
                .is_some_and(|current| {
                    current.acknowledged && current.generation == entry.generation
                })
            || requests.generations.get(&entry.connection_id) != Some(&entry.generation)
            || prefix_player(world, entry.connection_id, entry.controller_id)
                != Some(entry.player_id)
        {
            continue;
        }
        world
            .resource_mut::<FreezeDispatchResults>()
            .observe((entry.connection_id, request_id), entry);
        let _ = pipe.try_send(PlayerControlEvent::PrefixAck(PrefixAck {
            request_id,
            connection_id: entry.connection_id,
        }));
    }
    let failed = world
        .resource::<TicketedResults>()
        .pending
        .iter()
        .filter_map(|(&key, entry)| {
            (entry.kind == ControlRequestKind::Release
                && world
                    .resource::<FreezeDispatchResults>()
                    .prefix_completed
                    .get(&key)
                    .is_some_and(|result| *result != PrefixProofResult::ObservedPrefix))
            .then_some(*entry)
        })
        .collect::<Vec<_>>();
    for entry in failed {
        let _ = fail_ticketed_where(world, &pipe, |current| {
            current.connection_id == entry.connection_id
                && current.gate_request == entry.gate_request
        });
    }
}
