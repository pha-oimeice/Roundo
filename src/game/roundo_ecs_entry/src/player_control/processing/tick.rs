//! Fixed-tick settlement and publication of local AI input.

use super::*;

pub(in crate::player_control) fn process_local_ai(world: &mut World) {
    let now = authority_ingress_tick(world);
    maintain_authority(world, now);
    let mut inputs = ControllerTickInputs::default();
    while let Some(command) = world.resource_mut::<LocalAiQueue>().0.pop_front() {
        match command {
            LocalAiCommand::Settle(request) => {
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
                    world.resource_scope(
                        |world, mut admission: bevy::prelude::Mut<Admission>| {
                            if outcome == LocalAiOutcome::Failed {
                                let _ =
                                    admission.fail_transition(world, entry.controller, request);
                                admission.revoke_controller(entry.controller);
                            }
                            admission.take_terminal(world, entry.controller);
                        },
                    );
                    world
                        .resource_mut::<LocalAiTickets>()
                        .finish(request, outcome);
                } else {
                    let outcome = world.resource_scope(
                        |world, mut admission: bevy::prelude::Mut<Admission>| {
                            let outcome = admission
                                .commit_ai_acquire(
                                    world,
                                    entry.controller,
                                    entry.ai,
                                    request,
                                    now,
                                )
                                .map(LocalAiOutcome::Acquired)
                                .unwrap_or(LocalAiOutcome::Failed);
                            if outcome == LocalAiOutcome::Failed {
                                let _ =
                                    admission.fail_transition(world, entry.controller, request);
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
            LocalAiCommand::Submit {
                ai,
                controller,
                binding,
                input,
            } => {
                let admission = world.resource::<Admission>();
                let active = admission.submit_ai(world, controller, ai) == Ok(binding);
                let old_prefix = !active
                    && world.resource::<LocalAiTickets>().pending.iter().any(
                        |(&request, entry)| {
                            entry.release
                                && entry.ai == ai
                                && entry.controller == controller
                                && entry.old_binding == Some(binding)
                                && admission.draining_ai_binding(
                                    world, controller, ai, request, binding,
                                )
                        },
                    );
                if active || old_prefix {
                    inputs.queue(world, controller, input);
                }
            }
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
}

fn maintain_authority(world: &mut World, now: u64) {
    let reclaimed = world
        .resource_mut::<ControllerRegistry>()
        .take_reclaimed_controller_ids();
    if !reclaimed.is_empty() {
        let mut sequences = world.resource_mut::<ControllerInputSequences>();
        for controller_id in reclaimed {
            sequences.forget_controller(controller_id);
        }
    }
    // Snapshot candidates before running native read probes. No ticket or
    // authority mutation occurs until the same original selection is complete.
    let local_pending = world
        .resource::<LocalAiTickets>()
        .pending
        .iter()
        .map(|(&request, &entry)| (request, entry))
        .collect::<Vec<_>>();
    let local_expired = local_pending
        .into_iter()
        .filter(|(_, entry)| {
            entry.deadline <= now
                || !world
                    .resource::<ControllerRegistry>()
                    .is_ai_registered(entry.ai)
                || controller_intent_body(world, entry.controller).is_none()
        })
        .collect::<Vec<_>>();
    for (request, entry) in local_expired {
        if entry.release {
            neutralize_controller(world, entry.controller);
        }
        world
            .resource_mut::<LocalAiTickets>()
            .finish(request, LocalAiOutcome::Failed);
        world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
            let _ = admission.fail_transition(world, entry.controller, request);
            admission.take_terminal(world, entry.controller);
        });
    }
}
