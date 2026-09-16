//! Unified client/server player-controller domain.

mod block_interaction;
mod client;
mod input;
mod movement;
mod rotation;
mod server;
mod shared;

use bevy::prelude::Bundle;
pub use roundo_contracts::{ControllerCommand, PlayerControllerCommand};

// Re-export controller contracts so callers do not depend on subsystem layout.
pub use self::block_interaction::{
    BlockInteraction, BlockInteractionMessage, DestroyBlockController,
    DestroyBlockControllerAction, PlaceBlockController, PlaceBlockControllerAction,
};
pub use self::client::{
    ClientMarionetteCommand, ClientMarionetteEvent, ClientMarionetteInputSettings,
    ClientMarionetteIpc, ClientPlacedVoxelId, ClientPlayerController, MarionetteClientPlugin,
};
pub use self::input::{
    ClientInputBindings, DESTROY_BLOCK_INPUT_SLOT, InputBinding, InputDefinition, InputRegistry,
    InputRegistryError, MOVE_BACKWARD_INPUT_SLOT, MOVE_DOWN_INPUT_SLOT, MOVE_FORWARD_INPUT_SLOT,
    MOVE_LEFT_INPUT_SLOT, MOVE_RIGHT_INPUT_SLOT, MOVE_UP_INPUT_SLOT, PLACE_BLOCK_INPUT_SLOT,
    PhysicalInput, SPIRIT_CAMERA_INPUT_SLOT,
};
pub use self::movement::{Movement3D, Movement3DAction};
pub use self::rotation::{ROTATION_SYNC_INTERVAL_SECS, RotationSync};
pub use self::server::{
    ConnectionId, MarionetteServerPlugin, MarionetteServerSet, NetworkControllerTarget,
    ServerMarionetteCommand, ServerMarionetteIpc,
};
pub use self::shared::{ControllerAction, ControllerError, EventController};

#[derive(Bundle, Clone, Debug, Default)]
/// Authoritative controller state attached to each controlled player.
pub struct PlayerControllers {
    pub movement: Movement3D,
    pub destroy_block: DestroyBlockController,
    pub place_block: PlaceBlockController,
}

#[cfg(test)]
// Bundle composition is a public invariant consumed by server spawning.
mod tests {
    use super::*;

    #[test]
    fn player_bundle_contains_movement_and_block_controllers() {
        let mut world = bevy::prelude::World::new();
        let entity = world.spawn(PlayerControllers::default()).id();

        assert!(world.get::<Movement3D>(entity).is_some());
        assert!(world.get::<DestroyBlockController>(entity).is_some());
        assert!(world.get::<PlaceBlockController>(entity).is_some());
    }
}
