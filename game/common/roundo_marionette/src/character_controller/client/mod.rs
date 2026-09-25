//! Client controller state, input settings, and network-bridge contracts.

mod systems;

#[cfg(feature = "dev")]
use super::SpawnTestCreatureController;
use super::{DestroyBlockController, Movement3D, PlaceBlockController};
use bevy::prelude::{App, Entity, Message, Plugin, Resource, SystemSet};
pub use roundo_contracts::{
    ControllerControlState, ControllerId, ControllerOperationError, CreatureMotionSnapshot,
    PlayerControllerAccessSnapshot, PlayerControllerOperation, PlayerState,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
    LinearInterpolation,
};
pub(super) const PLAYER_INTERPOLATION_DURATION_SECS: f32 = 1.0 / 20.0;

/// Clonable endpoint for authoritative state input and controller-command output.
///
/// Clones share the plugin's two unbounded queues. Sending is non-blocking and
/// does not wait for ECS reconciliation or network forwarding.
pub type ClientMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ClientMarionetteCommand, ClientMarionetteEvent>;

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
    /// Starts a fresh controller session and invalidates all prior authority.
    BeginSession { epoch: u64 },
    /// Saves the opaque authoritative Creature snapshot for a later prediction slice.
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
    /// Requests the current Player's Controller Access projection.
    RequestPlayerControllerAccess,
    /// Explicitly requests exclusive control; never emitted by camera input.
    AcquireController { controller_id: ControllerId },
    /// Explicitly releases current control.
    ReleaseController { controller_id: ControllerId },
    /// Sends one sequenced input through an explicitly controlled Controller.
    SubmitControllerInput {
        controller_id: ControllerId,
        input: roundo_contracts::DirectedControllerInput,
    },
    /// Legacy compatibility path for commands outside this ticket's scope.
    UsePlayerController(roundo_contracts::PlayerControllerCommand),
}

/// Authoritative current-session Player Controller projection and explicit selection.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ClientPlayerControllerAccess {
    snapshot: Option<PlayerControllerAccessSnapshot>,
    movement: Option<ControllerId>,
    gaze: Option<ControllerId>,
    last_rejection: Option<(PlayerControllerOperation, ControllerOperationError)>,
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
    pub fn last_rejection(&self) -> Option<(PlayerControllerOperation, ControllerOperationError)> {
        self.last_rejection
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
    SessionStarted {
        epoch: u64,
    },
    Snapshot {
        epoch: u64,
        snapshot: CreatureMotionSnapshot,
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
/// The camera starts detached so joining a session preserves unrestricted free
/// flight. Spirit walking detaches camera motion from authoritative Creature
/// movement; returning from it explicitly binds the view to that Creature.
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
            spirit_walking: true,
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

/// Compact voxel ID selected from the immutable voxel Resource Registry by the
/// client composition root. Input routing does not parse Mod declarations.
#[derive(Resource, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientPlacedVoxelId(pub u32);

impl Default for ClientPlacedVoxelId {
    fn default() -> Self {
        Self(1)
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

#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ClientMarionetteSession(pub(super) Option<u64>);

#[derive(Resource, Clone, Copy, Debug, Default)]
pub(super) struct AuthoritativePlayerState(pub(super) Option<PlayerState>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct ClientControllerCommandState {
    pub(super) movement: Movement3D,
    pub(super) destroy_block: DestroyBlockController,
    pub(super) place_block: PlaceBlockController,
    #[cfg(feature = "dev")]
    pub(super) spawn_test_creature: SpawnTestCreatureController,
}

#[derive(Resource, Clone, Copy, Debug)]
pub(super) struct PlayerTranslationInterpolation(pub(super) LinearInterpolation<3>);

impl Default for PlayerTranslationInterpolation {
    fn default() -> Self {
        Self(LinearInterpolation::stationary(
            [0.0; 3],
            PLAYER_INTERPOLATION_DURATION_SECS,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_starts_with_a_detached_free_camera() {
        let controller = ClientPlayerController::default();

        assert!(controller.is_spirit_walking());
        assert!(controller.input_enabled());
    }
}
