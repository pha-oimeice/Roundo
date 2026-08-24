use super::RotationSyncMessage;
use bevy::prelude::{MessageReader, Quat, Query, Transform};

pub(crate) fn apply_rotation_sync(
    mut messages: MessageReader<RotationSyncMessage>,
    mut transforms: Query<&mut Transform>,
) {
    for message in messages.read() {
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
