//! Client controller state, input settings, and network-bridge contracts.

mod systems;

use super::{DestroyBlockController, Movement3D, PlaceBlockController};
use bevy::prelude::{App, Entity, Plugin, Resource};
pub use roundo_contracts::PlayerState;
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

/// Installs authoritative reconciliation followed by ordered local-input systems.
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
    /// Reconciles the camera against the latest complete player state.
    PlayerState(PlayerState),
    /// Clears authoritative and predicted state and exits spirit-walking mode.
    ClearPlayerState,
}

/// Validated local controller intent emitted toward the network adapter.
#[derive(Clone, Debug)]
pub enum ClientMarionetteEvent {
    /// Sends one sequenced movement, rotation, or block-interaction command.
    UsePlayerController(roundo_contracts::PlayerControllerCommand),
}

/// Camera binding and local-input authority for the controlled player.
///
/// Spirit walking detaches camera motion from authoritative player movement;
/// returning from it restores the latest authoritative pose.
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

#[derive(Resource, Clone, Copy, Debug, Default)]
pub(super) struct AuthoritativePlayerState(pub(super) Option<PlayerState>);

#[derive(Resource, Clone, Copy, Debug, Default)]
pub(super) struct ClientMovementPredictionState {
    pub(super) offset: bevy::prelude::Vec3,
}

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct ClientControllerCommandState {
    pub(super) movement: Movement3D,
    pub(super) destroy_block: DestroyBlockController,
    pub(super) place_block: PlaceBlockController,
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
