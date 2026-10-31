//! Reclaim stale authority and expire bounded books before reading commands.

use super::*;

pub(super) fn maintain_authority(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    now: u64,
) {
    let reclaimed = world
        .resource_mut::<ControllerRegistry>()
        .take_reclaimed_controller_ids();
    if !reclaimed.is_empty() {
        let mut sequences = world.resource_mut::<ControllerInputSequences>();
        for controller_id in reclaimed {
            sequences.forget_controller(controller_id);
        }
    }
    {
        let mut requests = world.resource_mut::<PrefixRequests>();
        while requests
            .deadlines
            .first_key_value()
            .is_some_and(|(&due, _)| due <= now)
        {
            let (_, ids) = requests.deadlines.pop_first().expect("due bucket");
            for key in ids {
                requests.entries.remove(&key);
            }
        }
    }
    {
        let tick = now;
        let due: Vec<_> = world
            .resource::<FreezeDispatchResults>()
            .pending
            .iter()
            .filter_map(|(&id, entry)| (entry.deadline <= tick).then_some(id))
            .collect();
        let mut results = world.resource_mut::<FreezeDispatchResults>();
        for id in due {
            results.finish(id, FreezeDispatchResult::TimedOut);
        }
    }
    let expired = world
        .resource::<TicketedResults>()
        .pending
        .iter()
        .filter_map(|(&id, &entry)| (entry.deadline <= now).then_some((id, entry)))
        .collect::<Vec<_>>();
    for (id, entry) in expired {
        if entry.kind == ControlRequestKind::Release {
            neutralize_controller(world, entry.controller_id);
        }
        world.resource_mut::<TicketedResults>().pending.remove(&id);
        world.resource_mut::<FreezeDispatchResults>().discard(id);
        world
            .resource_mut::<SettlementCertificates>()
            .discard(ControlRequestId(entry.gate_request.0));
        world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
            let _ = admission.fail_transition(world, entry.controller_id, entry.gate_request);
            admission.take_terminal(world, entry.controller_id);
        });
        let valid = world
            .resource::<PrefixRequests>()
            .generations
            .get(&entry.connection_id)
            == Some(&entry.generation)
            && player_for_connection(world, entry.connection_id) == Some(entry.player_id);
        ticketed_terminal(
            &pipe,
            entry.connection_id,
            id.1,
            entry.controller_id,
            if valid {
                ControlHandoffError::DeadlineElapsed
            } else {
                ControlHandoffError::NotReady
            },
        );
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
            if let Some(binding) = entry.old_binding {
                world.resource_mut::<LocalAiIngress>().discard_binding(
                    entry.ai,
                    entry.controller,
                    binding,
                );
            }
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
