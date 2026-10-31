//! Client controller state, input settings, and network-bridge contracts.

mod production;
mod systems;

use super::Movement3D;
#[cfg(feature = "dev")]
use super::SpawnTestCreatureController;
use bevy::prelude::{App, Entity, Message, Plugin, Resource, SystemSet};
pub use roundo_contracts::{
    ControllerControlState, ControllerId, ControllerOperationError, CreatureMotionSnapshot,
    PlayerControllerAccessSnapshot, PlayerControllerOperation, PlayerState,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::collections::VecDeque;
/// Maximum number of distinct recent ticket completions available for per-request observation.
///
/// Observations are diagnostic/UI feedback only; the lifecycle ticket book remains
/// the authority for request settlement.
pub const CONTROL_COMPLETION_OBSERVATION_CAPACITY: usize = 64;

/// Clonable endpoint for authoritative state input and controller-command output.
///
/// Clones share the plugin's two unbounded queues. Sending is non-blocking and
/// does not wait for ECS reconciliation or network forwarding.
pub type ClientMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ClientMarionetteCommand, ClientMarionetteEvent>;

/// Runtime-local event writer belonging to exactly the assembled Marionette App.
pub type ClientMarionetteEventIpc =
    CrossbeamThreadPipeEndpointB<ClientMarionetteCommand, ClientMarionetteEvent>;

/// Installs authoritative update routing followed by ordered local-input systems.
#[derive(Debug, Clone)]
pub struct MarionetteClientPlugin {
    pipe: CrossbeamThreadPipe<ClientMarionetteCommand, ClientMarionetteEvent>,
}

impl MarionetteClientPlugin {
    /// Creates a plugin with a fresh bidirectional command pipe.
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    /// Returns an endpoint sharing this plugin's command and event queues.
    pub fn ipc(&self) -> ClientMarionetteIpc {
        self.pipe.endpoint_a()
    }

    pub fn event_ipc(&self) -> ClientMarionetteEventIpc {
        self.pipe.endpoint_b()
    }
}

impl Default for MarionetteClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Authoritative controller-state update consumed by the client ECS.
#[derive(Clone, Debug)]
pub enum ClientMarionetteCommand {
    /// Explicit local request for the current Player's Access snapshot.
    RequestPlayerControllerAccess,
    /// Explicit local request to acquire one visible Controller.
    AcquireController { controller_id: ControllerId },
    /// Explicit local request to release one controlled Controller.
    ReleaseController { controller_id: ControllerId },
    /// Ticketed producer freeze; never sent by the legacy control path.
    Freeze {
        epoch: u64,
        freeze: roundo_contracts::control_handoff::Freeze,
    },
    /// Matched ticketed Acquire invitation; revokes the old producer and prediction.
    /// This is not Ready and does not grant control.
    ControlPreparing {
        epoch: u64,
        preparing: roundo_contracts::control_handoff::ControlPreparing,
    },
    TicketAcquired {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
        binding: roundo_contracts::ControllerBindingId,
    },
    TicketActivated {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
    },
    TicketReleased {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
    },
    /// Exactly-once terminal observation owned by the production bridge. This
    /// makes local dispatch failure and timeout visible without another caller
    /// racing to drain the ticket book.
    TicketCompleted {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
        outcome: ClientControlOutcome,
    },
    /// Starts a fresh controller session and invalidates all prior authority.
    BeginSession { epoch: u64 },
    /// Delivers an authoritative Creature snapshot to client reconciliation.
    CreatureMotionSnapshot(CreatureMotionSnapshot),
    /// Updates the legacy Presence baseline without moving a detached camera.
    PlayerState(PlayerState),
    /// Applies an epoch-scoped authoritative Controller Access snapshot.
    PlayerControllerAccessSnapshot {
        epoch: u64,
        snapshot: PlayerControllerAccessSnapshot,
    },
    /// Applies an epoch-scoped Controller operation success.
    ControllerAcquired {
        epoch: u64,
        controller_id: ControllerId,
    },
    ControllerReleased {
        epoch: u64,
        controller_id: ControllerId,
    },
    /// Records an epoch-scoped rejected operation for observable callers.
    ControllerOperationRejected {
        epoch: u64,
        operation: PlayerControllerOperation,
        error: ControllerOperationError,
    },
    /// Ends the matching controller session. Repeated or stale ends are harmless.
    EndSession { epoch: u64 },
}

/// Validated local controller intent emitted toward the network adapter.
#[derive(Clone, Debug)]
pub enum ClientMarionetteEvent {
    /// Ordered FIFO fence: old producer state is gone and this epoch owns input.
    SessionActivated { epoch: u64 },
    /// Requests the current Player's Controller Access projection.
    RequestPlayerControllerAccess,
    /// Explicitly requests exclusive control; never emitted by camera input.
    /// The epoch keeps a stale FIFO event from selecting in a replacement session.
    AcquireController {
        epoch: u64,
        controller_id: ControllerId,
    },
    /// Explicitly releases current control.
    ReleaseController { controller_id: ControllerId },
    /// Sends one sequenced input through an explicitly controlled Controller.
    SubmitControllerInput {
        controller_id: ControllerId,
        input: roundo_contracts::DirectedControllerInput,
    },
    /// Same outbound FIFO as accepted Submit events; no acknowledgement implied.
    DrainBarrier(roundo_contracts::control_handoff::DrainBarrier),
    /// Emitted only after the producer and prediction ECS stages both revoked
    /// their old state. The stream-0 bridge still must settle earlier FIFO items.
    PreparingCleaned {
        epoch: u64,
        preparing: roundo_contracts::control_handoff::ControlPreparing,
    },
    /// FIFO marker after applying the authenticated terminal, before new input.
    BindingClosureConfirmed {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
    },
    BindingActivated {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
    },
    /// Legacy compatibility path for commands outside this ticket's scope.
    UsePlayerController(roundo_contracts::PlayerControllerCommand),
}

/// Client-visible terminal outcome for one ticketed control request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientControlOutcome {
    Terminal(
        Result<
            roundo_contracts::control_handoff::ControlSuccess,
            roundo_contracts::control_handoff::ControlHandoffError,
        >,
    ),
    DispatchFailed,
    TimedOut,
}

/// Authoritative current-session Player Controller projection and explicit selection.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ClientPlayerControllerAccess {
    epoch: Option<u64>,
    frozen: production::ProducerFreeze,
    ticket_bindings: production::TicketBindings,
    snapshot: Option<PlayerControllerAccessSnapshot>,
    movement: Option<ControllerId>,
    gaze: Option<ControllerId>,
    last_rejection: Option<(PlayerControllerOperation, ControllerOperationError)>,
    last_control_result: Option<(
        roundo_contracts::control_handoff::ControlRequestId,
        ControllerId,
        ClientControlOutcome,
    )>,
    control_completion_count: u64,
    // Arrival-ordered recent observations. Repeated observations refresh their
    // position and increment their count; the oldest distinct request is evicted.
    control_completion_observations:
        VecDeque<(roundo_contracts::control_handoff::ControlRequestId, u64)>,
}

impl ClientPlayerControllerAccess {
    pub fn snapshot(&self) -> Option<&PlayerControllerAccessSnapshot> {
        self.snapshot.as_ref()
    }
    pub fn movement_controller(&self) -> Option<ControllerId> {
        self.movement
    }
    pub fn gaze_controller(&self) -> Option<ControllerId> {
        self.gaze
    }
    pub fn block_interaction_controller(&self) -> Option<ControllerId> {
        self.snapshot
            .as_ref()?
            .controllers
            .iter()
            .find_map(|entry| {
                (entry.intent_domain == roundo_contracts::ControllerIntentDomain::BlockInteraction
                    && entry.access_level == roundo_contracts::ControllerAccessLevel::ReadWrite)
                    .then_some(entry.controller_id)
            })
    }
    pub fn last_rejection(&self) -> Option<(PlayerControllerOperation, ControllerOperationError)> {
        self.last_rejection
    }
    /// Latest ticket completion, set once by the bridge owner for UI/selection
    /// feedback. A new selection/session replaces it.
    pub fn last_control_result(
        &self,
    ) -> Option<(
        roundo_contracts::control_handoff::ControlRequestId,
        ControllerId,
        ClientControlOutcome,
    )> {
        self.last_control_result
    }
    /// Number of ticket completions consumed in the current client session.
    /// This supports UI/test observation of exactly-once lifecycle delivery.
    pub fn control_completion_count(&self) -> u64 {
        self.control_completion_count
    }
    /// Number of distinct recent request observations retained for this session.
    /// This never exceeds [`CONTROL_COMPLETION_OBSERVATION_CAPACITY`].
    pub fn retained_control_completion_count(&self) -> usize {
        self.control_completion_observations.len()
    }
    /// Number of completions consumed for a retained recent request in this
    /// client session. Evicted and never-observed request IDs return zero.
    /// A terminal ticket is expected to contribute exactly one observation.
    pub fn control_completion_count_for(
        &self,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
    ) -> u64 {
        self.control_completion_observations
            .iter()
            .find_map(|(observed_id, count)| (*observed_id == request_id).then_some(*count))
            .unwrap_or(0)
    }
    pub(super) fn record_control_completion(
        &mut self,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
    ) {
        let count = self
            .control_completion_observations
            .iter()
            .position(|(observed_id, _)| *observed_id == request_id)
            .and_then(|index| {
                self.control_completion_observations
                    .remove(index)
                    .map(|(_, count)| count.saturating_add(1))
            })
            .unwrap_or(1);
        if self.control_completion_observations.len() == CONTROL_COMPLETION_OBSERVATION_CAPACITY {
            self.control_completion_observations.pop_front();
        }
        self.control_completion_observations
            .push_back((request_id, count));
    }
    pub fn is_controlled_by_self(&self, id: ControllerId) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.controllers.iter().any(|entry| {
                entry.controller_id == id
                    && entry.control_state == ControllerControlState::ControlledBySelf
            })
        })
    }
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Authority changes emitted by Marionette for Creature Prediction to consume.
#[derive(Message, Clone, Copy, Debug)]
pub enum CreatureAuthorityUpdate {
    /// Local prediction binding projection; only matched ticket terminals from
    /// the production bridge may activate a producer. This message is not authority.
    BindingTransition {
        epoch: u64,
        movement: Option<roundo_contracts::ControllerBindingId>,
        gaze: Option<roundo_contracts::ControllerBindingId>,
    },
    SessionStarted {
        epoch: u64,
    },
    Snapshot {
        epoch: u64,
        snapshot: CreatureMotionSnapshot,
    },
    BindingClosed {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
        movement: Option<roundo_contracts::ControllerBindingId>,
        gaze: Option<roundo_contracts::ControllerBindingId>,
    },
    BindingInstalled {
        epoch: u64,
        request_id: roundo_contracts::control_handoff::ControlRequestId,
        controller_id: ControllerId,
        movement: Option<roundo_contracts::ControllerBindingId>,
        gaze: Option<roundo_contracts::ControllerBindingId>,
    },
    PreparingRevoked {
        epoch: u64,
        preparing: roundo_contracts::control_handoff::ControlPreparing,
        movement: Option<roundo_contracts::ControllerBindingId>,
        gaze: Option<roundo_contracts::ControllerBindingId>,
    },
    SessionEnded {
        epoch: u64,
    },
}

/// Semantic intent that was accepted by the local transport adapter.
///
/// Creature Prediction retains these commands; Controller input owns only
/// validation, sequencing, and delivery.
#[derive(Message, Clone, Copy, Debug)]
pub struct LocallyRoutedControllerIntent(pub roundo_contracts::PlayerControllerCommand);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum MarionetteClientSet {
    Commands,
    Input,
}

/// Camera binding and local-input authority for the controlled player.
///
/// The camera starts attached to the controlled test Creature. Spirit walking
/// explicitly detaches camera motion from authoritative Creature movement.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientPlayerController {
    pub(super) camera: Option<Entity>,
    pub(super) input_enabled: bool,
    pub(super) spirit_walking: bool,
}

impl Default for ClientPlayerController {
    fn default() -> Self {
        Self {
            camera: None,
            input_enabled: true,
            spirit_walking: false,
        }
    }
}

impl ClientPlayerController {
    /// Replaces the camera entity used by controller systems.
    ///
    /// The entity is not validated here; systems skip camera mutation if it no
    /// longer exists or lacks the required components.
    pub fn bind_camera(&mut self, entity: Entity) {
        self.camera = Some(entity);
    }

    /// Returns the currently bound logical camera entity, if any.
    pub fn camera(&self) -> Option<Entity> {
        self.camera
    }

    /// Returns whether `entity` is the current logical camera binding.
    pub fn is_bound_to(&self, entity: Entity) -> bool {
        self.camera == Some(entity)
    }

    /// Returns whether camera movement is currently detached from the player.
    pub fn is_spirit_walking(&self) -> bool {
        self.spirit_walking
    }

    /// Enables or suppresses subsequent local controller input.
    ///
    /// Disabling input does not clear authoritative state, queued bridge events,
    /// camera binding, or the current spirit-walking mode.
    pub fn set_input_enabled(&mut self, enabled: bool) {
        self.input_enabled = enabled;
    }

    /// Returns whether controller systems currently accept local input.
    pub fn input_enabled(&self) -> bool {
        self.input_enabled
    }
}

/// Runtime input tuning consumed directly by movement and rotation systems.
///
/// This type does not validate either field; composition adapters must provide
/// finite values in their intended ranges.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientMarionetteInputSettings {
    /// Rotation radians applied per mouse-motion unit.
    pub mouse_sensitivity: f32,
    /// Spirit-camera movement speed in world units per second.
    pub camera_move_speed: f32,
    /// Enables local player movement prediction before authoritative correction.
    pub player_movement_prediction: bool,
}

impl Default for ClientMarionetteInputSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.002,
            camera_move_speed: 5.0,
            player_movement_prediction: false,
        }
    }
}

#[derive(Resource, Clone)]
pub(super) struct ClientPipeResource(
    pub(super) CrossbeamThreadPipeEndpointB<ClientMarionetteCommand, ClientMarionetteEvent>,
);

#[derive(Resource, Clone, Copy, Debug, Default)]
pub(super) struct AuthoritativePlayerState(pub(super) Option<PlayerState>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct ClientControllerCommandState {
    pub(super) block_sequence: u64,
    pub(super) movement: Movement3D,
    #[cfg(feature = "dev")]
    pub(super) spawn_test_creature: SpawnTestCreatureController,
}
