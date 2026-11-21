//! Unified client/server player-controller domain.

mod client;
mod input;
mod movement;
mod rotation;
mod server;
mod shared;
mod test_creature;

use bevy::prelude::Bundle;
pub use roundo_contracts::ControllerCommand;

// Re-export controller contracts so callers do not depend on subsystem layout.
pub use self::client::{
    ClientMarionetteInputSettings, ClientPlayerController, MarionetteClientPlugin,
    MarionetteClientSet,
};
pub use self::input::{
    ClientFunctionFrame, InputFunctionDefinition, InputFunctionRegistry,
    InputFunctionRegistryError, LOOK_FUNCTION, MOVE_BACKWARD_FUNCTION, MOVE_DOWN_FUNCTION,
    MOVE_FORWARD_FUNCTION, MOVE_LEFT_FUNCTION, MOVE_RIGHT_FUNCTION, MOVE_UP_FUNCTION,
};
pub use self::movement::{
    AcceptedMovementIntent, Movement3D, Movement3DAction, Movement3DMessage,
};
pub use self::rotation::{AcceptedGazeIntent, GazeController, GazeIntent, GazeIntentMessage};
pub use self::server::{MarionetteServerPlugin, MarionetteServerSet};
pub use self::shared::{ControllerAction, ControllerError, EventController};
pub use self::test_creature::{
    SpawnTestCreatureAction, SpawnTestCreatureController, SpawnTestCreatureControllerMessage,
    SpawnTestCreatureIntent,
};

#[derive(Bundle, Clone, Debug, Default)]
/// Authoritative controller state attached to each controlled player.
pub struct PlayerControllers {
    pub movement: Movement3D,
    pub spawn_test_creature: SpawnTestCreatureController,
}
