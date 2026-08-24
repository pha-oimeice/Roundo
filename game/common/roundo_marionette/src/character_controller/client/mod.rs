mod systems;

use super::{DestroyBlockController, Movement3D, PlaceBlockController};
use bevy::prelude::{App, Entity, Plugin, Resource};
pub use roundo_networking::protocol::PlayerState;
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
    LinearInterpolation,
};

pub use super::movement::{ClientKeyBindings, MovementAction};

pub(super) const PLAYER_INTERPOLATION_DURATION_SECS: f32 = 1.0 / 20.0;

pub type ClientMarionetteIpc =
    CrossbeamThreadPipeEndpointA<ClientMarionetteCommand, ClientMarionetteEvent>;

#[derive(Debug, Clone)]
pub struct MarionetteClientPlugin {
    pipe: CrossbeamThreadPipe<ClientMarionetteCommand, ClientMarionetteEvent>,
}

impl MarionetteClientPlugin {
    pub fn new() -> Self {
        Self {
            pipe: CrossbeamThreadPipe::new(),
        }
    }

    pub fn ipc(&self) -> ClientMarionetteIpc {
        self.pipe.endpoint_a()
    }
}

impl Default for MarionetteClientPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum ClientMarionetteCommand {
    PlayerState(PlayerState),
    ClearPlayerState,
}

#[derive(Clone, Debug)]
pub enum ClientMarionetteEvent {
    UsePlayerController(roundo_networking::protocol::PlayerControllerCommand),
}

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
    pub fn bind_camera(&mut self, entity: Entity) {
        self.camera = Some(entity);
    }

    pub fn camera(&self) -> Option<Entity> {
        self.camera
    }

    pub fn is_bound_to(&self, entity: Entity) -> bool {
        self.camera == Some(entity)
    }

    pub fn is_spirit_walking(&self) -> bool {
        self.spirit_walking
    }

    pub fn set_input_enabled(&mut self, enabled: bool) {
        self.input_enabled = enabled;
    }

    pub fn input_enabled(&self) -> bool {
        self.input_enabled
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientMarionetteInputSettings {
    pub mouse_sensitivity: f32,
    pub camera_move_speed: f32,
}

impl Default for ClientMarionetteInputSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.002,
            camera_move_speed: 5.0,
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
