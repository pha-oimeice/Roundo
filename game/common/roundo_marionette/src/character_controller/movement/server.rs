use super::{Movement3D, Movement3DMessage};
use bevy::prelude::{MessageReader, Query, Transform, Vec3};

pub(crate) fn apply_movement(
    mut messages: MessageReader<Movement3DMessage>,
    mut controllers: Query<(&mut Movement3D, &mut Transform)>,
) {
    for message in messages.read() {
        let Ok((mut controller, mut transform)) = controllers.get_mut(message.entity) else {
            continue;
        };
        let Ok(action) = controller.accept(message.command) else {
            continue;
        };
        transform.translation += Vec3::from_array(action.translation_delta);
    }
}
