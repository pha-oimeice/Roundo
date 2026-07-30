use crate::local_coordinate::data::{
    LocalCoordinate, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
};
use bevy::prelude::{
    App, Commands, FixedUpdate, MessageReader, Plugin, Query, ResMut, Resource, Update,
};

/// Loads shared voxel mutation and triangle-cache systems.
pub struct LocalCoordinateBasePlugin;

impl Plugin for LocalCoordinateBasePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GeometryRevisionCounter>()
            .add_message::<LocalCoordinateCRUDMessage>()
            .add_systems(FixedUpdate, apply_crud_messages)
            .add_systems(Update, rebuild_dirty_chunk_triangles);
    }
}

#[derive(Resource, Default)]
pub struct GeometryRevisionCounter {
    value: u64,
}

fn apply_crud_messages(
    mut commands: Commands,
    mut messages: MessageReader<LocalCoordinateCRUDMessage>,
    mut local_coordinates: Query<&mut LocalCoordinate>,
) {
    for message in messages.read() {
        match &message.0 {
            LocalCoordinateCRUDMessageEnum::Create { key, value } => {
                commands
                    .entity(*key)
                    .insert(LocalCoordinate::from_voxels(value.iter().copied()));
            }
            LocalCoordinateCRUDMessageEnum::Update { key, value } => {
                if let Ok(mut local_coordinate) = local_coordinates.get_mut(*key) {
                    local_coordinate.apply_voxels(value.iter().copied());
                }
            }
            LocalCoordinateCRUDMessageEnum::Delete { key } => {
                commands.entity(*key).remove::<LocalCoordinate>();
            }
            LocalCoordinateCRUDMessageEnum::Retrieve { .. } => {}
        }
    }
}

pub(crate) fn rebuild_dirty_chunk_triangles(
    mut local_coordinates: Query<&mut LocalCoordinate>,
    mut revision_counter: ResMut<GeometryRevisionCounter>,
) {
    for mut local_coordinate in &mut local_coordinates {
        if local_coordinate.rebuild_dirty_chunks() {
            revision_counter.value = revision_counter.value.wrapping_add(1);
            local_coordinate.geometry_revision = revision_counter.value;
        }
    }
}
