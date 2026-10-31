//! Owns ticket admission, producer handshake and release-barrier recognition.

use super::*;

pub(super) fn request(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    now: u64,
    connection_id: ConnectionId,
    request: ControlRequest,
) {
    // Authenticate before touching replay state. Unknown senders cannot
    // allocate namespaces, emit terminals, or poison other clients.
    let generation = world
        .resource::<PrefixRequests>()
        .generations
        .get(&connection_id)
        .copied();
    let player = player_for_connection(world, connection_id);
    let sole = player.is_some_and(|player| {
        world
            .resource::<PlayerRegistry>()
            .connections_for_player(player)
            .count()
            == 1
    });
    let (Some(generation), Some(player_id), true) = (generation, player, sole) else {
        return;
    };
    let authorized = world
        .resource::<PlayerRegistry>()
        .player_entity(player_id)
        .and_then(|entity| world.get::<Player>(entity))
        .is_some_and(|player| {
            player.controller_access(request.controller_id)
                == roundo_contracts::ControllerAccessLevel::ReadWrite
        });
    if !authorized || controller_intent_body(world, request.controller_id).is_none() {
        ticketed_terminal(
            pipe,
            connection_id,
            request.id,
            request.controller_id,
            ControlHandoffError::AccessDenied,
        );
        return;
    }
    let tick = now;
    let mut book = world.resource_mut::<TicketedResults>();
    if request.id.0 == 0
        || (!book.highest_ids.contains_key(&connection_id)
            && book.highest_ids.len() >= MAX_TICKETED_NAMESPACES)
    {
        return;
    }
    let highest = book
        .highest_ids
        .entry(connection_id)
        .or_insert((generation, 0));
    if generation < highest.0 || request.id.0 <= highest.1 {
        return;
    }
    highest.0 = generation;
    highest.1 = request.id.0;
    if book.pending.len() >= MAX_TICKETED_REQUESTS {
        ticketed_terminal(
            pipe,
            connection_id,
            request.id,
            request.controller_id,
            ControlHandoffError::Busy,
        );
        return;
    }
    let deadline = tick
        .checked_add(TICKETED_RETENTION_TICKS)
        .expect("ticket clock exhausted");
    drop(book);
    let access_revision = world
        .resource::<PlayerRegistry>()
        .player_entity(player_id)
        .and_then(|entity| world.get::<Player>(entity))
        .and_then(|player| player.controller_access_revision(request.controller_id))
        .unwrap_or(0);
    let result = world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
        let connection = admission
            .connection(connection_id)
            .ok_or(roundo_controller::authority_gate::GateError::Unauthorized)?;
        let old_binding = if request.kind == ControlRequestKind::Release {
            Some(admission.submit_player(world, request.controller_id, connection)?)
        } else {
            None
        };
        let time = Deadline {
            now: tick,
            deadline,
        };
        let gate_request = match request.kind {
            ControlRequestKind::Acquire => {
                admission.begin_player_acquire(world, request.controller_id, connection, time)?
            }
            ControlRequestKind::Release => {
                admission.begin_player_release(world, request.controller_id, connection, time)?
            }
        };
        Ok::<_, roundo_controller::authority_gate::GateError>((
            connection,
            gate_request,
            old_binding,
        ))
    });
    match result {
        Ok((connection, gate_request, old_binding)) => {
            world.resource_mut::<TicketedResults>().pending.insert(
                (connection_id, request.id),
                TicketedEntry {
                    connection_id,
                    generation,
                    player_id,
                    controller_id: request.controller_id,
                    deadline,
                    preparing_tick: tick,
                    gate_request,
                    connection,
                    kind: request.kind,
                    old_binding,
                    access_revision,
                },
            );
            if request.kind == ControlRequestKind::Acquire {
                let _ = pipe.try_send(PlayerControlEvent::Preparing {
                    connection_id,
                    preparing: ControlPreparing {
                        request_id: request.id,
                        controller_id: request.controller_id,
                    },
                });
            } else {
                // Submit was closed above. Queue admission is not producer delivery.
                let mut results = world.resource_mut::<FreezeDispatchResults>();
                results.pending.insert(
                    (connection_id, request.id),
                    PendingFreeze {
                        connection_id,
                        controller_id: request.controller_id,
                        player_id,
                        generation,
                        deadline,
                        queued: false,
                        reported: false,
                        observed: false,
                    },
                );
                drop(results);
                if pipe
                    .try_send(PlayerControlEvent::Freeze {
                        connection_id,
                        freeze: Freeze {
                            request_id: request.id,
                            controller_id: request.controller_id,
                        },
                    })
                    .is_err()
                {
                    world.resource_mut::<FreezeDispatchResults>().finish(
                        (connection_id, request.id),
                        FreezeDispatchResult::SendFailed,
                    );
                }
            }
        }
        Err(_) => ticketed_terminal(
            pipe,
            connection_id,
            request.id,
            request.controller_id,
            ControlHandoffError::Busy,
        ),
    }
}

pub(super) fn ready(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    now: u64,
    connection_id: ConnectionId,
    ready: Ready,
) {
    let key = (connection_id, ready.request_id);
    let Some(entry) = world
        .resource::<TicketedResults>()
        .pending
        .get(&key)
        .copied()
    else {
        return;
    };
    let tick = now;
    if entry.kind != ControlRequestKind::Acquire
        || entry.controller_id != ready.controller_id
        || tick <= entry.preparing_tick
    {
        return;
    }
    let result = world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
        admission.commit_player_acquire(
            world,
            entry.controller_id,
            entry.connection,
            entry.gate_request,
            tick,
            true,
        )
    });
    if let Ok(binding) = result {
        world.resource_mut::<TicketedResults>().pending.remove(&key);
        world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
            admission.take_terminal(world, entry.controller_id);
        });
        // A new binding is an independent sequence namespace.
        ControllerTickInputs::begin_binding(world, entry.controller_id);
        let _ = pipe.try_send(PlayerControlEvent::TicketedTerminal {
            connection_id,
            terminal: ControlTerminal {
                request_id: ready.request_id,
                controller_id: ready.controller_id,
                outcome: Ok(ControlSuccess::Acquired(binding.into())),
            },
        });
    }
}

pub(super) fn freeze_dispatch_reported(
    world: &mut World,
    now: u64,
    connection_id: ConnectionId,
    request_id: ControlRequestId,
    queued: bool,
) {
    let tick = now;
    let generations = &world.resource::<PrefixRequests>().generations;
    let results = world.resource::<FreezeDispatchResults>();
    let Some(entry) = results.pending.get(&(connection_id, request_id)).copied() else {
        return;
    };
    if entry.reported {
        return;
    }
    let outcome = if entry.deadline <= tick {
        FreezeDispatchResult::TimedOut
    } else if entry.connection_id != connection_id
        || generations.get(&connection_id) != Some(&entry.generation)
        || (prefix_player(world, connection_id, entry.controller_id) != Some(entry.player_id)
            && !world
                .resource::<TicketedResults>()
                .pending
                .get(&(connection_id, request_id))
                .is_some_and(|ticket| {
                    ticket.kind == ControlRequestKind::Release
                        && ticket.player_id == entry.player_id
                        && ticket.controller_id == entry.controller_id
                        && ticket.generation == entry.generation
                }))
    {
        FreezeDispatchResult::Disconnected
    } else if queued {
        FreezeDispatchResult::Queued
    } else {
        FreezeDispatchResult::SendFailed
    };
    world
        .resource_mut::<FreezeDispatchResults>()
        .finish((connection_id, request_id), outcome);
    if outcome != FreezeDispatchResult::Queued {
        world
            .resource_mut::<PrefixRequests>()
            .remove_id((connection_id, request_id));
    }
}

/// Legacy prefix validation stays with the FIFO owner; an ignored ticketed
/// release must never fall through and acquire legacy proof.
pub(super) enum ReleaseBarrier {
    NotTicketed,
    Ignored,
    Accepted(PrefixRequest),
}

pub(super) fn release_barrier(
    world: &World,
    now: u64,
    connection_id: ConnectionId,
    barrier: DrainBarrier,
) -> ReleaseBarrier {
    if let Some(entry) = world
        .resource::<TicketedResults>()
        .pending
        .get(&(connection_id, barrier.request_id))
        .copied()
        .filter(|entry| entry.kind == ControlRequestKind::Release)
    {
        let tick = now;
        let dispatched = world
            .resource::<FreezeDispatchResults>()
            .pending
            .get(&(connection_id, barrier.request_id))
            .is_some_and(|pending| {
                pending.reported
                    && pending.queued
                    && pending.connection_id == connection_id
                    && pending.generation == entry.generation
            });
        if entry.controller_id == barrier.controller_id
            && tick < entry.deadline
            && dispatched
            && world
                .resource::<PrefixRequests>()
                .generations
                .get(&connection_id)
                == Some(&entry.generation)
            && player_for_connection(world, connection_id) == Some(entry.player_id)
            && world
                .resource::<PlayerRegistry>()
                .connections_for_player(entry.player_id)
                .count()
                == 1
        {
            return ReleaseBarrier::Accepted(PrefixRequest {
                connection_id,
                player_id: entry.player_id,
                generation: entry.generation,
                controller_id: entry.controller_id,
                deadline: entry.deadline,
                acknowledged: true,
            });
        }
        return ReleaseBarrier::Ignored;
    }
    ReleaseBarrier::NotTicketed
}
