//! Sole command FIFO owner; phase ordering is authority-significant.

use super::*;
mod maintenance;
mod sessions;
mod settlement;
mod ticketed;

pub(in crate::player_control) fn process_player_control(world: &mut World) {
    let now = authority_ingress_tick(world);
    let pipe = world.resource::<PlayerControlPipe>().0.clone();
    maintenance::maintain_authority(world, &pipe, now);
    let mut pending_acks = Vec::new();
    let mut inputs = ControllerTickInputs::default();
    while let Some(command) = pipe.try_receive() {
        match command {
            PlayerControlCommand::Inventory {
                connection_id,
                request,
            } => {
                let result = super::inventory::read_inventory(world, connection_id, request);
                let _ = pipe.try_send(PlayerControlEvent::Inventory {
                    connection_id,
                    reply: roundo_contracts::inventory::InventoryReply { request, result },
                });
            }
            PlayerControlCommand::Debug {
                connection_id,
                request,
            } => {
                // Public diagnostic capability for any current authenticated player.
                // UI resource, visibility and dev discovery levels are not authority.
                if player_for_connection(world, connection_id).is_none() {
                    continue;
                }
                let result = world
                    .get_resource_mut::<crate::debug::DebugIngress>()
                    .map(|mut ingress| ingress.admit(connection_id, request.clone()))
                    .unwrap_or_else(|| {
                        Err(roundo_contracts::debug::DebugFailure::new(
                            "unavailable",
                            "诊断能力未安装",
                        ))
                    });
                if let Err(error) = result {
                    let _ = pipe.try_send(PlayerControlEvent::Debug {
                        connection_id,
                        reply: roundo_contracts::debug::DebugReply {
                            request_id: request.request_id,
                            result: Err(error),
                        },
                    });
                }
            }
            PlayerControlCommand::LocalAiSettle { request } => {
                let Some(entry) = world
                    .resource::<LocalAiTickets>()
                    .pending
                    .get(&request)
                    .copied()
                else {
                    continue;
                };
                if now >= entry.deadline {
                    continue;
                }
                if entry.release {
                    // The marker is the exact FIFO boundary. Account for every
                    // accepted old-binding command in the sequence high-water before
                    // discarding its aggregated intent and clearing held action.
                    let valid = entry.old_binding.is_some_and(|binding| {
                        world.resource::<Admission>().draining_ai_binding(
                            world,
                            entry.controller,
                            entry.ai,
                            request,
                            binding,
                        )
                    });
                    let discarded = inputs.discard_ai_prefix(world, entry.controller, valid);
                    let valid = valid && neutralize_controller(world, entry.controller);
                    if let Some(binding) = entry.old_binding {
                        world.resource_mut::<LocalAiIngress>().discard_binding(
                            entry.ai,
                            entry.controller,
                            binding,
                        );
                    }
                    let outcome = if valid {
                        world.resource_scope(
                            |world, mut admission: bevy::prelude::Mut<Admission>| {
                                admission
                                    .commit_ai_release(
                                        world,
                                        entry.controller,
                                        entry.ai,
                                        request,
                                        now,
                                        entry.old_binding.unwrap(),
                                    )
                                    .map(|()| LocalAiOutcome::Released {
                                        movement_sequence: discarded.movement,
                                        gaze_sequence: discarded.gaze,
                                    })
                                    .unwrap_or(LocalAiOutcome::Failed)
                            },
                        )
                    } else {
                        LocalAiOutcome::Failed
                    };
                    world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
                        if outcome == LocalAiOutcome::Failed {
                            let _ = admission.fail_transition(world, entry.controller, request);
                            admission.revoke_controller(entry.controller);
                        }
                        admission.take_terminal(world, entry.controller);
                    });
                    world
                        .resource_mut::<LocalAiTickets>()
                        .finish(request, outcome);
                } else {
                    let outcome = world.resource_scope(
                        |world, mut admission: bevy::prelude::Mut<Admission>| {
                            let outcome = admission
                                .commit_ai_acquire(world, entry.controller, entry.ai, request, now)
                                .map(LocalAiOutcome::Acquired)
                                .unwrap_or(LocalAiOutcome::Failed);
                            if outcome == LocalAiOutcome::Failed {
                                let _ = admission.fail_transition(world, entry.controller, request);
                            }
                            admission.take_terminal(world, entry.controller);
                            outcome
                        },
                    );
                    if matches!(outcome, LocalAiOutcome::Acquired(_)) {
                        ControllerTickInputs::begin_binding(world, entry.controller);
                    }
                    world
                        .resource_mut::<LocalAiTickets>()
                        .finish(request, outcome);
                }
            }
            PlayerControlCommand::LocalAiSubmit(envelope) => {
                // Consume a matching, one-shot reservation before inspecting the gate.
                // A raw IPC clone/replay or a guessed binding never extends the prefix.
                if !world.resource_mut::<LocalAiIngress>().consume(&envelope) {
                    continue;
                }
                let admission = world.resource::<Admission>();
                let active = admission.submit_ai(world, envelope.controller, envelope.ai)
                    == Ok(envelope.binding);
                let old_prefix = !active
                    && world.resource::<LocalAiTickets>().pending.iter().any(
                        |(&request, entry)| {
                            entry.release
                                && entry.ai == envelope.ai
                                && entry.controller == envelope.controller
                                && entry.old_binding == Some(envelope.binding)
                                && admission.draining_ai_binding(
                                    world,
                                    envelope.controller,
                                    envelope.ai,
                                    request,
                                    envelope.binding,
                                )
                        },
                    );
                if active || old_prefix {
                    inputs.queue(world, envelope.controller, envelope.input);
                }
            }
            PlayerControlCommand::TicketedRequest {
                connection_id,
                request,
            } => ticketed::request(world, &pipe, now, connection_id, request),
            PlayerControlCommand::TicketedReady {
                connection_id,
                ready,
            } => ticketed::ready(world, &pipe, now, connection_id, ready),
            PlayerControlCommand::Connected {
                connection_id,
                external_identity,
            } => sessions::connected(world, &pipe, &mut inputs, connection_id, external_identity),
            PlayerControlCommand::Disconnected { connection_id } => {
                sessions::disconnected(world, &pipe, &mut inputs, connection_id);
            }
            PlayerControlCommand::RequestAccess { connection_id } => {
                match world
                    .run_system_once_with(reads::project_access, connection_id)
                    .expect("Access projection has its declared read-only parameters")
                {
                    reads::AccessProjection::UnknownConnection => {
                        let _ = pipe.try_send(PlayerControlEvent::Rejected {
                            connection_id,
                            operation: PlayerControllerOperation::RequestAccess,
                            error: ControllerOperationError::AccessDenied,
                        });
                    }
                    reads::AccessProjection::Unavailable => {}
                    reads::AccessProjection::Snapshot(snapshot) => {
                        let _ = pipe.try_send(PlayerControlEvent::AccessSnapshot {
                            connection_id,
                            snapshot,
                        });
                    }
                }
            }
            PlayerControlCommand::Acquire {
                connection_id,
                controller_id,
            } => {
                let _ = pipe.try_send(PlayerControlEvent::Rejected {
                    connection_id,
                    operation: PlayerControllerOperation::Acquire { controller_id },
                    error: ControllerOperationError::AccessDenied,
                });
            }
            PlayerControlCommand::Release {
                connection_id,
                controller_id,
            } => {
                let _ = pipe.try_send(PlayerControlEvent::Rejected {
                    connection_id,
                    operation: PlayerControllerOperation::Release { controller_id },
                    error: ControllerOperationError::AccessDenied,
                });
            }
            PlayerControlCommand::RegisterPrefixRequest {
                connection_id,
                request,
            } => {
                let Some(player_id) = prefix_player(world, connection_id, request.controller_id)
                else {
                    continue;
                };
                let mut requests = world.resource_mut::<PrefixRequests>();
                if request.id.0 == 0
                    || request.id.0 <= *requests.highest_ids.get(&connection_id).unwrap_or(&0)
                    || requests.entries.len() >= MAX_PREFIX_REQUESTS
                {
                    continue;
                }
                let Some(&generation) = requests.generations.get(&connection_id) else {
                    continue;
                };
                let deadline = now
                    .checked_add(PREFIX_RETENTION_TICKS)
                    .expect("prefix clock exhausted");
                requests.highest_ids.insert(connection_id, request.id.0);
                requests
                    .deadlines
                    .entry(deadline)
                    .or_default()
                    .insert((connection_id, request.id));
                requests.entries.insert(
                    (connection_id, request.id),
                    PrefixRequest {
                        connection_id,
                        player_id,
                        generation,
                        controller_id: request.controller_id,
                        deadline,
                        acknowledged: false,
                    },
                );
            }
            PlayerControlCommand::InitiateFreeze {
                connection_id,
                request,
            } => {
                let player = prefix_player(world, connection_id, request.controller_id);
                let generation = world
                    .resource::<PrefixRequests>()
                    .generations
                    .get(&connection_id)
                    .copied();
                let mut results = world.resource_mut::<FreezeDispatchResults>();
                // Unreserved commands cannot register a ticket or spoof a dispatch.
                if results.reserved.get(&(connection_id, request.id)) != Some(&request) {
                    continue;
                }
                results.reserved.remove(&(connection_id, request.id));
                let result = if request.id.0 == 0 || request.kind != ControlRequestKind::Release {
                    Some(FreezeDispatchResult::Invalid)
                } else if player.is_none() || generation.is_none() {
                    Some(FreezeDispatchResult::Unavailable)
                } else {
                    None
                };
                if let Some(result) = result {
                    results
                        .completed
                        .insert((connection_id, request.id), result);
                    results.prefix_completed.insert(
                        (connection_id, request.id),
                        match result {
                            FreezeDispatchResult::Invalid => PrefixProofResult::Invalid,
                            _ => PrefixProofResult::Unavailable,
                        },
                    );
                    continue;
                }
                // Enroll only this trusted request; the public wire barrier cannot create proof.
                drop(results);
                let mut requests = world.resource_mut::<PrefixRequests>();
                if request.id.0 <= *requests.highest_ids.get(&connection_id).unwrap_or(&0)
                    || requests.entries.len() >= MAX_PREFIX_REQUESTS
                {
                    drop(requests);
                    let mut results = world.resource_mut::<FreezeDispatchResults>();
                    results.completed.insert(
                        (connection_id, request.id),
                        FreezeDispatchResult::Unavailable,
                    );
                    results
                        .prefix_completed
                        .insert((connection_id, request.id), PrefixProofResult::Unavailable);
                    continue;
                }
                let deadline = now
                    .checked_add(PREFIX_RETENTION_TICKS)
                    .expect("prefix clock exhausted");
                requests.highest_ids.insert(connection_id, request.id.0);
                requests
                    .deadlines
                    .entry(deadline)
                    .or_default()
                    .insert((connection_id, request.id));
                requests.entries.insert(
                    (connection_id, request.id),
                    PrefixRequest {
                        connection_id,
                        player_id: player.unwrap(),
                        generation: generation.unwrap(),
                        controller_id: request.controller_id,
                        deadline,
                        acknowledged: false,
                    },
                );
                drop(requests);
                let mut results = world.resource_mut::<FreezeDispatchResults>();
                results.pending.insert(
                    (connection_id, request.id),
                    PendingFreeze {
                        connection_id,
                        controller_id: request.controller_id,
                        player_id: player.unwrap(),
                        generation: generation.unwrap(),
                        deadline,
                        queued: false,
                        reported: false,
                        observed: false,
                    },
                );
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
                    results.finish(
                        (connection_id, request.id),
                        FreezeDispatchResult::SendFailed,
                    );
                }
            }
            PlayerControlCommand::FreezeDispatchReported {
                connection_id,
                request_id,
                queued,
            } => ticketed::freeze_dispatch_reported(world, now, connection_id, request_id, queued),
            PlayerControlCommand::DrainBarrier {
                connection_id,
                barrier,
            } => {
                match ticketed::release_barrier(world, now, connection_id, barrier) {
                    ticketed::ReleaseBarrier::Accepted(entry) => {
                        pending_acks.push((barrier.request_id, entry));
                        continue;
                    }
                    ticketed::ReleaseBarrier::Ignored => continue,
                    ticketed::ReleaseBarrier::NotTicketed => {}
                }
                let requests = world.resource::<PrefixRequests>();
                let Some(&entry) = requests.entries.get(&(connection_id, barrier.request_id))
                else {
                    continue;
                };
                if entry.acknowledged
                    || entry.connection_id != connection_id
                    || entry.controller_id != barrier.controller_id
                    || entry.deadline <= now
                    || requests.generations.get(&connection_id) != Some(&entry.generation)
                    || prefix_player(world, connection_id, barrier.controller_id)
                        != Some(entry.player_id)
                {
                    continue;
                }
                // Deferred until all same-tick pending aggregation is settled below.
                pending_acks.push((barrier.request_id, entry));
                world
                    .resource_mut::<PrefixRequests>()
                    .entries
                    .get_mut(&(connection_id, barrier.request_id))
                    .unwrap()
                    .acknowledged = true;
            }
            PlayerControlCommand::Submit {
                connection_id,
                controller_id,
                input,
            } => {
                let authorized = world
                    .resource::<Admission>()
                    .connection(connection_id)
                    .and_then(|connection| {
                        world
                            .resource::<Admission>()
                            .submit_player(world, controller_id, connection)
                            .ok()
                    })
                    .is_some();
                let accepted = authorized && inputs.queue(world, controller_id, input);
                if !accepted {
                    let _ = pipe.try_send(PlayerControlEvent::Rejected {
                        connection_id,
                        operation: PlayerControllerOperation::SubmitInput { controller_id },
                        error: ControllerOperationError::AccessDenied,
                    });
                }
            }
        }
    }
    // A raw Access mutation still advances its revision. Revalidate every active
    // gate before publication so revocation cannot retain a held command.
    let active = world
        .query::<&roundo_controller::Controller>()
        .iter(world)
        .filter_map(|controller| {
            world
                .resource::<Admission>()
                .active_player(controller.id())
                .map(|(connection, _)| (controller.id(), connection))
        })
        .collect::<Vec<_>>();
    for (id, connection_id) in active {
        let valid = world
            .resource::<Admission>()
            .connection(connection_id)
            .is_some_and(|connection| {
                world
                    .resource::<Admission>()
                    .submit_player(world, id, connection)
                    .is_ok()
            });
        if !valid {
            neutralize_controller(world, id);
            inputs.discard(id);
            world.resource_mut::<Admission>().revoke_controller(id);
        }
    }
    // Raw local AI removal cannot leave held input active: gate truth and
    // registration are rechecked before the motion schedule observes intents.
    let active_ai = world
        .query::<&roundo_controller::Controller>()
        .iter(world)
        .filter_map(|controller| {
            world
                .resource::<Admission>()
                .active_ai(world, controller.id())
                .map(|ai| (controller.id(), ai))
        })
        .collect::<Vec<_>>();
    for (id, ai) in active_ai {
        if world
            .resource::<Admission>()
            .submit_ai(world, id, ai)
            .is_err()
        {
            neutralize_controller(world, id);
            inputs.discard(id);
            world.resource_mut::<Admission>().revoke_controller(id);
        }
    }
    inputs.publish(world);
    settlement::settle_pending_prefixes(world, &pipe, now, pending_acks);
}
