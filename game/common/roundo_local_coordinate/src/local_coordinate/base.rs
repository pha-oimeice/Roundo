use crate::local_coordinate::data::{
    LocalCoordinate, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
};
use crate::local_coordinate::transform::configure_transform_derivation;
use crate::local_coordinate::virtual_chunk::{VirtualChunkIndex, rebuild_virtual_chunk_index};
use bevy::prelude::{
    App, Commands, FixedUpdate, IntoScheduleConfigs, MessageReader, Plugin, Query, SystemSet,
    Update,
};
use bevy::transform::TransformPlugin;

/// Loads shared voxel mutation and triangle-cache systems.
pub struct LocalCoordinateBasePlugin;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum LocalCoordinateSet {
    ApplyCrud,
}

const MAX_CHUNK_TRIANGULATIONS_PER_COORDINATE_PER_FRAME: usize = 8;

impl Plugin for LocalCoordinateBasePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransformPlugin>() {
            app.add_plugins(TransformPlugin);
        }
        configure_transform_derivation(app);
        app.init_resource::<VirtualChunkIndex>()
            .add_message::<LocalCoordinateCRUDMessage>()
            .add_systems(
                FixedUpdate,
                apply_crud_messages.in_set(LocalCoordinateSet::ApplyCrud),
            )
            .add_systems(
                FixedUpdate,
                rebuild_virtual_chunk_index.after(apply_crud_messages),
            )
            .add_systems(Update, rebuild_dirty_chunk_triangles);
    }
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

pub(crate) fn rebuild_dirty_chunk_triangles(mut local_coordinates: Query<&mut LocalCoordinate>) {
    for mut local_coordinate in &mut local_coordinates {
        local_coordinate
            .rebuild_dirty_chunks_with_limit(MAX_CHUNK_TRIANGULATIONS_PER_COORDINATE_PER_FRAME);
    }
}
