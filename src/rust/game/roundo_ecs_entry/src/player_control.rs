//! Server composition seam for Player identity and Controller wire operations.

mod processing;

pub(crate) use processing::ControllerInputSequences;

use processing::{neutralize_controller, process_player_control};

use crate::authority_admission::Admission;
use bevy::prelude::{App, FixedUpdate, IntoScheduleConfigs, Plugin, Resource, SystemSet, World};
use roundo_contracts::{
    ConnectionId, ControllerControlState, ControllerId, ControllerIntentDomain,
    ControllerOperationError, DirectedControllerInput, PlayerControllerAccess,
    PlayerControllerAccessSnapshot, PlayerControllerOperation, PlayerId,
    control_handoff::{
        ControlHandoffError, ControlPreparing, ControlRequest, ControlRequestId,
        ControlRequestKind, ControlSuccess, ControlTerminal, DrainBarrier, Freeze, PrefixAck,
        Ready,
    },
};
use roundo_controller::authority_gate::{
    AdmittedConnection, BindingId, Deadline, GateError, RequestId,
};
use roundo_controller::{
    AiControllerId, ControllerLifecyclePlugin, ControllerOwner, ControllerRegistry,
    release_all_control,
};
use roundo_creature::{
    CreatureLifeDataPlugin, FactoryMotionRoute, GazeAction, GazeCommandEvent, MovementAction,
    MovementCommandState,
};
use roundo_life::{
    AuthoritativeFixedTick, ConceptBodyHandle, LifeFixedSet, LifeHandle, LifeRuntimePlugin,
};
use roundo_player::{ExternalIdentityKey, Player, PlayerRegistry, find_or_create_player};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::{BTreeMap, HashMap, HashSet};

pub type PlayerControlServerIpc =
    CrossbeamThreadPipeEndpointA<PlayerControlCommand, PlayerControlEvent>;

#[derive(Clone, Debug)]
pub enum PlayerControlCommand {
    Inventory {
        connection_id: ConnectionId,
        request: roundo_contracts::inventory::InventoryRequest,
    },
    Debug {
        connection_id: ConnectionId,
        request: roundo_contracts::debug::DebugRequest,
    },
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
    LocalAiSettle {
        request: RequestId,
    },
    /// Sealed trusted local command; callers cannot construct its ingress permit.
    LocalAiSubmit(LocalAiEnqueued),
    Submit {
        connection_id: ConnectionId,
        controller_id: ControllerId,
        input: DirectedControllerInput,
    },
    /// Authenticated callback identity is supplied by the server bridge.
    TicketedRequest {
        connection_id: ConnectionId,
        request: ControlRequest,
    },
    TicketedReady {
        connection_id: ConnectionId,
        ready: Ready,
    },
    /// Trusted server-only admission; never constructed from a client wire message.
    RegisterPrefixRequest {
        connection_id: ConnectionId,
        request: ControlRequest,
    },
    DrainBarrier {
        connection_id: ConnectionId,
        barrier: DrainBarrier,
    },
    /// Only trusted server composition may enqueue this; never decoded from wire.
    InitiateFreeze {
        connection_id: ConnectionId,
        request: ControlRequest,
    },
    FreezeDispatchReported {
        connection_id: ConnectionId,
        request_id: ControlRequestId,
        queued: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerControlEvent {
    Inventory {
        connection_id: ConnectionId,
        reply: roundo_contracts::inventory::InventoryReply,
    },
    Debug {
        connection_id: ConnectionId,
        reply: roundo_contracts::debug::DebugReply,
    },
    PrefixAck(PrefixAck),
    TicketedTerminal {
        connection_id: ConnectionId,
        terminal: ControlTerminal,
    },
    Freeze {
        connection_id: ConnectionId,
        freeze: Freeze,
    },
    /// Emitted only by the future gate-backed Acquire transaction.
    Preparing {
        connection_id: ConnectionId,
        preparing: ControlPreparing,
    },
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
pub(crate) struct PlayerControlPipe(
    pub(crate) CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
);

const MAX_LOCAL_AI_INGRESS: usize = 64;

/// Opaque local IPC payload. Its permit and payload can only be created by
/// `local_ai_submit`, never decoded from a network DTO or minted by IPC callers.
#[derive(Clone, Debug)]
pub struct LocalAiEnqueued {
    permit: u128,
    ai: AiControllerId,
    controller: ControllerId,
    binding: BindingId,
    input: DirectedControllerInput,
}

#[derive(Resource)]
struct LocalAiIngress {
    next_permit: u128,
    pending: HashMap<u128, LocalAiEnqueued>,
}
impl Default for LocalAiIngress {
    fn default() -> Self {
        Self {
            next_permit: 1,
            pending: HashMap::new(),
        }
    }
}
impl LocalAiIngress {
    fn reserve(
        &mut self,
        ai: AiControllerId,
        controller: ControllerId,
        binding: BindingId,
        input: DirectedControllerInput,
    ) -> Result<LocalAiEnqueued, GateError> {
        if self.pending.len() >= MAX_LOCAL_AI_INGRESS {
            return Err(GateError::PendingWork);
        }
        let following = self
            .next_permit
            .checked_add(1)
            .ok_or(GateError::IdExhausted)?;
        let envelope = LocalAiEnqueued {
            permit: self.next_permit,
            ai,
            controller,
            binding,
            input,
        };
        self.next_permit = following;
        self.pending.insert(envelope.permit, envelope.clone());
        Ok(envelope)
    }
    fn consume(&mut self, envelope: &LocalAiEnqueued) -> bool {
        let matches = self.pending.get(&envelope.permit).is_some_and(|saved| {
            saved.ai == envelope.ai
                && saved.controller == envelope.controller
                && saved.binding == envelope.binding
                && match (saved.input, envelope.input) {
                    (
                        DirectedControllerInput::Movement3D(a),
                        DirectedControllerInput::Movement3D(b),
                    ) => {
                        a.sequence == b.sequence
                            && a.action
                                .direction
                                .into_iter()
                                .zip(b.action.direction)
                                .all(|(a, b)| a.to_bits() == b.to_bits())
                    }
                    (DirectedControllerInput::Gaze(a), DirectedControllerInput::Gaze(b)) => {
                        a.sequence == b.sequence
                            && a.action.yaw_delta.to_bits() == b.action.yaw_delta.to_bits()
                            && a.action.pitch_delta.to_bits() == b.action.pitch_delta.to_bits()
                    }
                    _ => false,
                }
        });
        if matches {
            self.pending.remove(&envelope.permit);
        }
        matches
    }
    fn discard_binding(
        &mut self,
        ai: AiControllerId,
        controller: ControllerId,
        binding: BindingId,
    ) {
        self.pending.retain(|_, entry| {
            entry.ai != ai || entry.controller != controller || entry.binding != binding
        });
    }
}

const MAX_LOCAL_AI_TICKETS: usize = 64;
const LOCAL_AI_DEADLINE_TICKS: u64 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalAiOutcome {
    Acquired(BindingId),
    /// Highest accepted old-binding input explicitly discarded at the FIFO marker.
    /// These are local settlement evidence, never Player wire ACKs.
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

/// Trusted server-local App seam; call only with the IPC paired to this World.
pub fn local_ai_request(
    world: &mut World,
    ipc: &PlayerControlServerIpc,
    ai: AiControllerId,
    controller: ControllerId,
    release: bool,
) -> Result<RequestId, GateError> {
    let book = world.resource::<LocalAiTickets>();
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
    if ipc
        .try_send(PlayerControlCommand::LocalAiSettle { request })
        .is_err()
    {
        if let Some(binding) = old_binding {
            world
                .resource_mut::<LocalAiIngress>()
                .discard_binding(ai, controller, binding);
        }
        world
            .resource_mut::<LocalAiTickets>()
            .finish(request, LocalAiOutcome::Failed);
        world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
            let _ = admission.fail_transition(world, controller, request);
            admission.take_terminal(world, controller);
        });
    }
    Ok(request)
}

/// Trusted server-local typed Submit through the same live authority gate.
pub fn local_ai_submit(
    world: &mut World,
    ipc: &PlayerControlServerIpc,
    ai: AiControllerId,
    controller: ControllerId,
    input: DirectedControllerInput,
) -> Result<(), GateError> {
    let binding = world
        .resource::<Admission>()
        .submit_ai(world, controller, ai)?;
    let envelope = world
        .resource_mut::<LocalAiIngress>()
        .reserve(ai, controller, binding, input)?;
    if ipc
        .try_send(PlayerControlCommand::LocalAiSubmit(envelope.clone()))
        .is_err()
    {
        world
            .resource_mut::<LocalAiIngress>()
            .pending
            .remove(&envelope.permit);
        return Err(GateError::PendingWork);
    }
    Ok(())
}

const MAX_PREFIX_REQUESTS: usize = 64;
// Server-owned retention; never derived from the client-local request deadline.
const PREFIX_RETENTION_TICKS: u64 = 64;
const MAX_FREEZE_REQUESTS: usize = 64;
const MAX_TICKETED_REQUESTS: usize = 64;
const MAX_TICKETED_NAMESPACES: usize = 64;
const MAX_QUEUED_TICKETED_TERMINALS: usize = 64;
const TICKETED_RETENTION_TICKS: u64 = 64;

#[derive(Clone, Copy)]
struct TicketedEntry {
    connection_id: ConnectionId,
    generation: u64,
    player_id: PlayerId,
    controller_id: ControllerId,
    deadline: u64,
    preparing_tick: u64,
    gate_request: RequestId,
    connection: AdmittedConnection,
    kind: ControlRequestKind,
    old_binding: Option<BindingId>,
    access_revision: u64,
}

/// Single ECS FIFO owner. Physical ConnectionId is never reused in this process
/// (network allocator rejects exhaustion); disconnect discards its replay state.
/// Terminal events still queued for that ID cannot target a future connection.
#[derive(Resource, Default)]
struct TicketedResults {
    highest_ids: HashMap<ConnectionId, (u64, u128)>,
    pending: HashMap<(ConnectionId, ControlRequestId), TicketedEntry>,
}

fn ticketed_terminal(
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    connection_id: ConnectionId,
    request_id: ControlRequestId,
    controller_id: ControllerId,
    error: ControlHandoffError,
) {
    // Only terminal notifications are throttled. Legacy events retain their
    // existing unbounded semantics. Drop on saturation; the client times out.
    // FixedUpdate is the sole sender of these terminal events.
    if pipe.queued_outbound() >= MAX_QUEUED_TICKETED_TERMINALS {
        return;
    }
    let _ = pipe.try_send(PlayerControlEvent::TicketedTerminal {
        connection_id,
        terminal: ControlTerminal {
            request_id,
            controller_id,
            outcome: Err(error),
        },
    });
}

fn fail_ticketed_where(
    world: &mut World,
    pipe: &CrossbeamThreadPipeEndpointB<PlayerControlCommand, PlayerControlEvent>,
    predicate: impl Fn(&TicketedEntry) -> bool,
) -> Vec<ControllerId> {
    let mut book = world.resource_mut::<TicketedResults>();
    let failed = book
        .pending
        .iter()
        .filter_map(|(&id, &entry)| predicate(&entry).then_some((id, entry)))
        .collect::<Vec<_>>();
    for (id, _) in &failed {
        book.pending.remove(id);
    }
    drop(book);
    let mut revoked = Vec::new();
    for (id, entry) in failed {
        if entry.kind == ControlRequestKind::Release {
            neutralize_controller(world, entry.controller_id);
            revoked.push(entry.controller_id);
        }
        world.resource_mut::<FreezeDispatchResults>().discard(id);
        world
            .resource_mut::<SettlementCertificates>()
            .discard(ControlRequestId(entry.gate_request.0));
        world.resource_scope(|world, mut admission: bevy::prelude::Mut<Admission>| {
            if admission
                .fail_transition(world, entry.controller_id, entry.gate_request)
                .is_ok()
            {
                admission.take_terminal(world, entry.controller_id);
            }
        });
        ticketed_terminal(
            pipe,
            entry.connection_id,
            id.1,
            entry.controller_id,
            ControlHandoffError::NotReady,
        );
    }
    revoked
}

/// A queued result means only local transport admission, never delivery or drain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreezeDispatchResult {
    Queued,
    Invalid,
    Duplicate,
    Unavailable,
    Full,
    SendFailed,
    TimedOut,
    Disconnected,
}

/// Per-connection ECS-observed prefix only; not producer-stop or full drain proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefixProofResult {
    ObservedPrefix,
    Invalid,
    Unavailable,
    SendFailed,
    TimedOut,
    Disconnected,
}

/// Privately issued, one-shot proof. A prefix alone cannot produce this: the
/// gate must first become the ECS input authority and its old binding must be
/// verified while held input is neutralized in the same FixedUpdate.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SettlementCertificate {
    connection: AdmittedConnection,
    player_id: PlayerId,
    controller_id: ControllerId,
    request_id: RequestId,
    old_binding: BindingId,
}

impl SettlementCertificate {
    pub(crate) fn matches(
        &self,
        connection: AdmittedConnection,
        player: PlayerId,
        controller: ControllerId,
        request: RequestId,
        old_binding: BindingId,
    ) -> bool {
        self.connection == connection
            && self.player_id == player
            && self.controller_id == controller
            && self.request_id == request
            && self.old_binding == old_binding
    }
}

/// Certificates are issued only after the ECS FIFO has settled old input and
/// neutralized the held intent in the same FixedUpdate.
#[derive(Resource, Default)]
pub struct SettlementCertificates {
    issued: HashMap<ControlRequestId, SettlementCertificate>,
}

impl SettlementCertificates {
    fn try_consume(
        &mut self,
        connection: AdmittedConnection,
        player_id: PlayerId,
        controller_id: ControllerId,
        request_id: RequestId,
        old_binding: BindingId,
    ) -> Option<SettlementCertificate> {
        let id = ControlRequestId(request_id.0);
        if self.issued.get(&id).is_some_and(|certificate| {
            *certificate
                == SettlementCertificate {
                    connection,
                    player_id,
                    controller_id,
                    request_id,
                    old_binding,
                }
        }) {
            self.issued.remove(&id)
        } else {
            None
        }
    }

    pub(crate) fn discard(&mut self, id: ControlRequestId) {
        self.issued.remove(&id);
    }

    fn discard_where(&mut self, predicate: impl Fn(&SettlementCertificate) -> bool) {
        self.issued.retain(|_, certificate| !predicate(certificate));
    }
}

#[derive(Clone, Copy)]
struct PendingFreeze {
    connection_id: ConnectionId,
    controller_id: ControllerId,
    player_id: PlayerId,
    generation: u64,
    deadline: u64,
    queued: bool,
    reported: bool,
    observed: bool,
}

type FreezeKey = (ConnectionId, ControlRequestId);

/// ECS-owned bounded result book; the outbound bridge cannot consume owner results.
/// The trusted caller polls after FixedUpdate; taking a result is one-shot.
#[derive(Resource, Default)]
pub struct FreezeDispatchResults {
    highest_ids: HashMap<ConnectionId, u128>,
    reserved: HashMap<FreezeKey, ControlRequest>,
    pending: HashMap<FreezeKey, PendingFreeze>,
    completed: HashMap<FreezeKey, FreezeDispatchResult>,
    prefix_completed: HashMap<FreezeKey, PrefixProofResult>,
}

impl FreezeDispatchResults {
    /// Trusted caller reserves bounded result capacity before joining the ECS FIFO.
    /// Full/duplicate are immediate errors, not accepted tickets.
    pub fn try_initiate(
        &mut self,
        ipc: &PlayerControlServerIpc,
        connection_id: ConnectionId,
        request: ControlRequest,
    ) -> Result<(), FreezeDispatchResult> {
        if request.id.0 == 0 {
            return Err(FreezeDispatchResult::Invalid);
        }
        if request.id.0 <= *self.highest_ids.get(&connection_id).unwrap_or(&0) {
            return Err(FreezeDispatchResult::Duplicate);
        }
        if self.reserved.len()
            + self.pending.len()
            + self.completed.len()
            + self.prefix_completed.len()
            >= MAX_FREEZE_REQUESTS
        {
            return Err(FreezeDispatchResult::Full);
        }
        self.highest_ids.insert(connection_id, request.id.0);
        self.reserved.insert((connection_id, request.id), request);
        if ipc
            .try_send(PlayerControlCommand::InitiateFreeze {
                connection_id,
                request,
            })
            .is_err()
        {
            self.reserved.remove(&(connection_id, request.id));
            self.completed.insert(
                (connection_id, request.id),
                FreezeDispatchResult::SendFailed,
            );
            self.prefix_completed
                .insert((connection_id, request.id), PrefixProofResult::SendFailed);
        }
        Ok(())
    }

    /// Trusted owner abandons a closed transaction; late dispatch/barrier cannot
    /// create a result for it. PrefixRequests separately expires its bounded entry.
    pub(crate) fn discard(&mut self, key: FreezeKey) {
        self.reserved.remove(&key);
        self.pending.remove(&key);
        self.completed.remove(&key);
        self.prefix_completed.remove(&key);
    }

    pub fn try_take_result(&mut self, key: FreezeKey) -> Option<FreezeDispatchResult> {
        self.completed.remove(&key)
    }

    pub fn try_take_prefix_result(&mut self, key: FreezeKey) -> Option<PrefixProofResult> {
        self.prefix_completed.remove(&key)
    }

    fn finish(&mut self, key: FreezeKey, result: FreezeDispatchResult) {
        if let Some(entry) = self.pending.get_mut(&key) {
            if !entry.reported {
                entry.reported = true;
                self.completed.insert(key, result);
            }
        }
        if result != FreezeDispatchResult::Queued {
            self.finish_prefix(
                key,
                match result {
                    FreezeDispatchResult::Invalid | FreezeDispatchResult::Duplicate => {
                        PrefixProofResult::Invalid
                    }
                    FreezeDispatchResult::Unavailable | FreezeDispatchResult::Full => {
                        PrefixProofResult::Unavailable
                    }
                    FreezeDispatchResult::SendFailed => PrefixProofResult::SendFailed,
                    FreezeDispatchResult::TimedOut => PrefixProofResult::TimedOut,
                    FreezeDispatchResult::Disconnected => PrefixProofResult::Disconnected,
                    FreezeDispatchResult::Queued => unreachable!(),
                },
            );
        } else if self.pending.get(&key).is_some_and(|entry| entry.observed) {
            self.finish_prefix(key, PrefixProofResult::ObservedPrefix);
        } else if let Some(entry) = self.pending.get_mut(&key) {
            entry.queued = true;
        }
    }

    fn finish_prefix(&mut self, key: FreezeKey, result: PrefixProofResult) {
        if self.pending.remove(&key).is_some() {
            self.prefix_completed.insert(key, result);
        }
    }

    fn observe(&mut self, key: FreezeKey, entry: PrefixRequest) {
        if let Some(pending) = self.pending.get_mut(&key)
            && pending.connection_id == entry.connection_id
            && pending.controller_id == entry.controller_id
            && pending.player_id == entry.player_id
            && pending.generation == entry.generation
        {
            if pending.queued {
                self.finish_prefix(key, PrefixProofResult::ObservedPrefix);
            } else {
                pending.observed = true;
            }
        }
    }

    fn fail_where(&mut self, predicate: impl Fn(&PendingFreeze) -> bool) {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(&id, entry)| predicate(entry).then_some(id))
            .collect();
        for key in ids {
            self.finish(key, FreezeDispatchResult::Disconnected);
        }
    }
}

#[derive(Clone, Copy)]
struct PrefixRequest {
    connection_id: ConnectionId,
    player_id: PlayerId,
    generation: u64,
    controller_id: ControllerId,
    deadline: u64,
    acknowledged: bool,
}

/// Server-local prefix correlation, not authority settlement. A high-water ID
/// rejects replay after bounded retention; wire input cannot directly register an entry.
#[derive(Resource, Default)]
struct PrefixRequests {
    next_generation: u64,
    generations: HashMap<ConnectionId, u64>,
    highest_ids: HashMap<ConnectionId, u128>,
    entries: HashMap<FreezeKey, PrefixRequest>,
    deadlines: BTreeMap<u64, HashSet<FreezeKey>>,
}

impl PrefixRequests {
    fn remove_id(&mut self, key: FreezeKey) {
        if let Some(entry) = self.entries.remove(&key) {
            let bucket = self
                .deadlines
                .get_mut(&entry.deadline)
                .expect("indexed deadline");
            bucket.remove(&key);
            if bucket.is_empty() {
                self.deadlines.remove(&entry.deadline);
            }
        }
    }

    fn remove_where(&mut self, predicate: impl Fn(&PrefixRequest) -> bool) {
        let ids = self
            .entries
            .iter()
            .filter_map(|(&key, entry)| predicate(entry).then_some(key))
            .collect::<Vec<_>>();
        for key in ids {
            let entry = self.entries.remove(&key).expect("indexed request");
            let bucket = self
                .deadlines
                .get_mut(&entry.deadline)
                .expect("indexed deadline");
            bucket.remove(&key);
            if bucket.is_empty() {
                self.deadlines.remove(&entry.deadline);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub(crate) enum PlayerControlServerSet {
    Commands,
}

impl Plugin for PlayerControlServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<CreatureLifeDataPlugin>() {
            app.add_plugins(CreatureLifeDataPlugin);
        }
        if !app.is_plugin_added::<roundo_block_interaction::BlockInteractionLifeDataPlugin>() {
            app.add_plugins(roundo_block_interaction::BlockInteractionLifeDataPlugin);
        }
        if !app.is_plugin_added::<LifeRuntimePlugin>() {
            app.add_plugins(LifeRuntimePlugin::default());
        }
        if !app.is_plugin_added::<ControllerLifecyclePlugin>() {
            app.add_plugins(ControllerLifecyclePlugin);
        }
        app.configure_sets(
            FixedUpdate,
            PlayerControlServerSet::Commands.after(LifeFixedSet::ExpireEvents),
        )
        .insert_resource(PlayerControlPipe(self.pipe.endpoint_b()))
        .init_resource::<ControllerInputSequences>()
        .init_resource::<PrefixRequests>()
        .init_resource::<FreezeDispatchResults>()
        .init_resource::<SettlementCertificates>()
        .init_resource::<TicketedResults>()
        .init_resource::<LocalAiTickets>()
        .init_resource::<LocalAiIngress>()
        .insert_resource(Admission::new(TICKETED_RETENTION_TICKS))
        .add_systems(
            FixedUpdate,
            process_player_control.in_set(PlayerControlServerSet::Commands),
        );
    }
}

pub(crate) fn authority_ingress_tick(world: &World) -> u64 {
    world.resource::<AuthoritativeFixedTick>().get()
}
