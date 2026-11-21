//! Trusted fixed-tick local AI input into native Creature commands.
mod processing;
use crate::authority_admission::Admission;
use bevy::prelude::{
    App, FixedUpdate, IntoScheduleConfigs, Plugin, Resource, SystemSet, World,
};
use processing::{ControllerInputSequences, process_local_ai};
use roundo_contracts::{ControllerCommand, ControllerId, GazeIntent, Movement3DAction};
use roundo_controller::{
    AiControllerId, ControllerLifecyclePlugin, ControllerRegistry,
    authority_gate::{BindingId, Deadline, GateError, RequestId},
};
use roundo_creature::{
    CreatureLifeDataPlugin, FactoryMotionRoute, GazeAction, GazeCommandEvent, MovementAction,
    MovementCommandState,
};
use roundo_life::{
    AuthoritativeFixedTick, ConceptBodyHandle, LifeFixedSet, LifeHandle, LifeRuntimePlugin,
};
use std::collections::{BTreeMap, HashMap, VecDeque};

const MAX_LOCAL_AI_INGRESS: usize = 64;

/// Input produced by an AI in this World for its bound Controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LocalAiInput {
    Movement3D(ControllerCommand<Movement3DAction>),
    Gaze(ControllerCommand<GazeIntent>),
}

#[derive(Resource, Default)]
struct LocalAiQueue(VecDeque<LocalAiCommand>);

enum LocalAiCommand {
    Settle(RequestId),
    Submit {
        ai: AiControllerId,
        controller: ControllerId,
        binding: BindingId,
        input: LocalAiInput,
    },
}

const MAX_LOCAL_AI_TICKETS: usize = 64;
const LOCAL_AI_DEADLINE_TICKS: u64 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalAiOutcome {
    Acquired(BindingId),
    /// Highest accepted old-binding input explicitly discarded at the FIFO marker.
    Released {
        movement_sequence: Option<u64>,
        gaze_sequence: Option<u64>,
    },
    Failed,
}

#[derive(Clone, Copy)]
struct LocalAiPending {
    ai: AiControllerId,
    controller: ControllerId,
    deadline: u64,
    release: bool,
    old_binding: Option<BindingId>,
}

/// Trusted ECS-local book: pending and unread terminal results share capacity.
#[derive(Resource, Default)]
pub struct LocalAiTickets {
    pending: HashMap<RequestId, LocalAiPending>,
    completed: HashMap<RequestId, LocalAiOutcome>,
}
impl LocalAiTickets {
    pub fn take(&mut self, request: RequestId) -> Option<LocalAiOutcome> {
        self.completed.remove(&request)
    }
    fn finish(&mut self, request: RequestId, result: LocalAiOutcome) {
        if self.pending.remove(&request).is_some() {
            self.completed.insert(request, result);
        }
    }
}

/// Trusted server-local App seam; settles at the fixed-tick boundary.
pub fn local_ai_request(
    world: &mut World,
    ai: AiControllerId,
    controller: ControllerId,
    release: bool,
) -> Result<RequestId, GateError> {
    let book = world.resource::<LocalAiTickets>();
    if world.resource::<LocalAiQueue>().0.len() >= MAX_LOCAL_AI_INGRESS {
        return Err(GateError::PendingWork);
    }
    if book.pending.len() + book.completed.len() >= MAX_LOCAL_AI_TICKETS {
        return Err(GateError::PendingWork);
    }
    let now = authority_ingress_tick(world);
    let deadline = now
        .checked_add(LOCAL_AI_DEADLINE_TICKS)
        .ok_or(GateError::Deadline)?;
    let time = Deadline { now, deadline };
    let (request, old_binding) =
        world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
            if release {
                let old = admission.submit_ai(world, controller, ai)?;
                let request = admission.begin_ai_release(world, controller, ai, time)?;
                Ok::<_, GateError>((request, Some(old)))
            } else {
                Ok((
                    admission.begin_ai_acquire(world, controller, ai, time)?,
                    None,
                ))
            }
        })?;
    world.resource_mut::<LocalAiTickets>().pending.insert(
        request,
        LocalAiPending {
            ai,
            controller,
            deadline,
            release,
            old_binding,
        },
    );
    // Acquire's FIFO marker is the local readiness barrier; Release already closed
    // admission synchronously before this marker can enter the controller FIFO.
    world
        .resource_mut::<LocalAiQueue>()
        .0
        .push_back(LocalAiCommand::Settle(request));
    Ok(request)
}

/// Trusted server-local typed Submit through the same live authority gate.
pub fn local_ai_submit(
    world: &mut World,
    ai: AiControllerId,
    controller: ControllerId,
    input: LocalAiInput,
) -> Result<(), GateError> {
    let binding = world
        .resource::<Admission>()
        .submit_ai(world, controller, ai)?;
    let mut queue = world.resource_mut::<LocalAiQueue>();
    if queue.0.len() >= MAX_LOCAL_AI_INGRESS {
        return Err(GateError::PendingWork);
    }
    queue.0.push_back(LocalAiCommand::Submit {
        ai,
        controller,
        binding,
        input,
    });
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub(crate) struct LocalAiControlSet;

pub struct LocalAiControlPlugin;

impl Plugin for LocalAiControlPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<CreatureLifeDataPlugin>() {
            app.add_plugins(CreatureLifeDataPlugin);
        }
        if !app.is_plugin_added::<LifeRuntimePlugin>() {
            app.add_plugins(LifeRuntimePlugin::default());
        }
        if !app.is_plugin_added::<ControllerLifecyclePlugin>() {
            app.add_plugins(ControllerLifecyclePlugin);
        }
        app.configure_sets(
            FixedUpdate,
            LocalAiControlSet.after(LifeFixedSet::ExpireEvents),
        )
        .init_resource::<ControllerInputSequences>()
        .init_resource::<LocalAiTickets>()
        .init_resource::<LocalAiQueue>()
        .insert_resource(Admission::new(LOCAL_AI_DEADLINE_TICKS))
        .add_systems(FixedUpdate, process_local_ai.in_set(LocalAiControlSet));
    }
}

fn authority_ingress_tick(world: &World) -> u64 {
    world.resource::<AuthoritativeFixedTick>().get()
}
