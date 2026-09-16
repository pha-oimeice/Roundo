//! Client prediction and authoritative synchronization for view rotation.

mod client;
mod server;

use bevy::prelude::{Entity, Message};
pub use roundo_contracts::RotationSync;

#[cfg(test)]
pub(super) use self::client::apply_camera_rotation;
pub(super) use self::client::{ClientRotationSyncState, rotate_camera, synchronize_rotation};
pub(super) use self::server::apply_rotation_sync;

/// Minimum interval between client rotation snapshots.
pub const ROTATION_SYNC_INTERVAL_SECS: f32 = 0.1;

#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(super) struct RotationSyncMessage {
    pub entity: Entity,
    pub sync: RotationSync,
}
