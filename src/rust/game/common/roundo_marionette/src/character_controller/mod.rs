//! Unified client/server player-controller domain.

mod block_interaction;
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
    ClientInputFrame, DESTROY_BLOCK_INPUT_SLOT, InputDefinition, InputRegistry, InputRegistryError,
    LOOK_INPUT_SLOT, MOVE_BACKWARD_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT, MOVE_FORWARD_INPUT_SLOT,
    MOVE_LEFT_INPUT_SLOT, MOVE_RIGHT_INPUT_SLOT, MOVE_UP_INPUT_SLOT, PLACE_BLOCK_INPUT_SLOT,
    SPAWN_TEST_CREATURE_INPUT_SLOT, SPIRIT_CAMERA_INPUT_SLOT,
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
