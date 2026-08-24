mod block_interaction;
mod client;
mod movement;
mod rotation;
mod server;
mod shared;

use bevy::prelude::Bundle;
pub use roundo_networking::protocol::{ControllerCommand, PlayerControllerCommand};

pub use self::block_interaction::{
    BlockInteraction, BlockInteractionMessage, DestroyBlockController,
    DestroyBlockControllerAction, PlaceBlockController, PlaceBlockControllerAction,
};
pub use self::client::{
    ClientKeyBindings, ClientMarionetteCommand, ClientMarionetteEvent,
    ClientMarionetteInputSettings, ClientMarionetteIpc, ClientPlayerController,
    MarionetteClientPlugin, MovementAction,
};
pub use self::movement::{Movement3D, Movement3DAction};
pub use self::rotation::{ROTATION_SYNC_INTERVAL_SECS, RotationSync};
pub use self::server::{
    ConnectionId, MarionetteServerPlugin, MarionetteServerSet, NetworkControllerTarget,
    ServerMarionetteCommand, ServerMarionetteIpc,
};
pub use self::shared::{ControllerAction, ControllerError, EventController};

#[derive(Bundle, Clone, Debug, Default)]
pub struct PlayerControllers {
    pub movement: Movement3D,
    pub destroy_block: DestroyBlockController,
    pub place_block: PlaceBlockController,
}

#[cfg(test)]
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
