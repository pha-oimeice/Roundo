//! Unified client/server player-controller domain.

mod client;
mod input;
mod movement;
mod rotation;
mod server;
mod shared;
mod test_creature;

use bevy::prelude::Bundle;
pub use roundo_contracts::{ControllerCommand, PlayerControllerCommand};

// Re-export controller contracts so callers do not depend on subsystem layout.
pub use self::client::{
    ClientControlOutcome, ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteEventIpc,
    ClientMarionetteInputSettings, ClientMarionetteIpc, ClientPlayerController,
    ClientPlayerControllerAccess, CreatureAuthorityUpdate, LocallyRoutedControllerIntent,
    MarionetteClientPlugin, MarionetteClientSet,
};
pub use self::input::{
    ClientFunctionFrame, InputFunctionDefinition, InputFunctionRegistry,
    InputFunctionRegistryError, LOOK_FUNCTION, MOVE_BACKWARD_FUNCTION, MOVE_DOWN_FUNCTION,
    MOVE_FORWARD_FUNCTION, MOVE_LEFT_FUNCTION, MOVE_RIGHT_FUNCTION, MOVE_UP_FUNCTION,
    SPAWN_TEST_CREATURE_FUNCTION, SPIRIT_CAMERA_FUNCTION,
};
pub use self::movement::{AcceptedMovementIntent, Movement3D, Movement3DAction};
pub use self::rotation::{
    AcceptedGazeIntent, GazeController, GazeIntent, ROTATION_SYNC_INTERVAL_SECS,
};
pub use self::server::{
    ConnectionId, MarionetteServerPlugin, MarionetteServerSet, NetworkControllerTarget,
    ServerMarionetteCommand, ServerMarionetteEvent, ServerMarionetteIpc,
    ServerMarionetteSnapshotSender,
};
pub use self::shared::{ControllerAction, ControllerError, EventController};
pub use self::test_creature::{
    SpawnTestCreatureAction, SpawnTestCreatureController, SpawnTestCreatureIntent,
};

#[derive(Bundle, Clone, Debug, Default)]
/// Authoritative controller state attached to each controlled player.
pub struct PlayerControllers {
    pub movement: Movement3D,
    pub spawn_test_creature: SpawnTestCreatureController,
}
