mod client;
mod server;

use bevy::prelude::{Entity, Message};
pub use roundo_networking::protocol::RotationSync;

#[cfg(test)]
pub(super) use self::client::apply_camera_rotation;
pub(super) use self::client::{ClientRotationSyncState, rotate_camera, synchronize_rotation};
pub(super) use self::server::apply_rotation_sync;

pub const ROTATION_SYNC_INTERVAL_SECS: f32 = 0.1;

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct RotationSyncMessage {
    pub entity: Entity,
    pub sync: RotationSync,
}
