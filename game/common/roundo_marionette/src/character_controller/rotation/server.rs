//! Server-side validation and application of rotation snapshots.

use super::RotationSyncMessage;
use bevy::prelude::{MessageReader, Quat, Query, Transform};

/// Normalizes finite nonzero rotations before updating entity transforms.
pub(crate) fn apply_rotation_sync(
    mut messages: MessageReader<RotationSyncMessage>,
    mut transforms: Query<&mut Transform>,
) {
    let pending_messages = messages.read();
    for message in pending_messages {
        let rotation = Quat::from_array(message.sync.rotation);
        if !rotation.is_finite() || rotation.length_squared() <= f32::EPSILON {
            continue;
        }
        let Ok(mut transform) = transforms.get_mut(message.entity) else {
            continue;
        };
        transform.rotation = rotation.normalize();
    }
}
