//! Local camera controller and input tuning.
mod systems;

use bevy::prelude::{Entity, Resource, SystemSet};

pub struct MarionetteClientPlugin;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum MarionetteClientSet {
    Input,
}

/// Camera binding and local-input authority for the controlled player.
///
/// Input modifies the bound camera in this World only.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ClientPlayerController {
    pub(super) camera: Option<Entity>,
    pub(super) input_enabled: bool,
}

impl Default for ClientPlayerController {
    fn default() -> Self {
        Self {
            camera: None,
            input_enabled: true,
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

    /// Enables or suppresses subsequent local controller input.
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
    /// Free-camera movement speed in world units per second.
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
