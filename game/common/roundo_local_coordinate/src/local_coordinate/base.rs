use crate::local_coordinate::data::{
    LocalCoordinate, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
};
use crate::local_coordinate::transform::configure_transform_derivation;
use crate::local_coordinate::virtual_chunk::{VirtualChunkIndex, rebuild_virtual_chunk_index};
use crate::{AtomicVoxelRegistry, EMPTY_VOXEL_ID};
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
    RebuildIndex,
}

impl Plugin for LocalCoordinateBasePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransformPlugin>() {
            app.add_plugins(TransformPlugin);
        }
        configure_transform_derivation(app);
        app.init_resource::<AtomicVoxelRegistry>()
            .init_resource::<VirtualChunkIndex>()
            .add_message::<LocalCoordinateCRUDMessage>()
            .add_systems(
                FixedUpdate,
                apply_crud_messages.in_set(LocalCoordinateSet::ApplyCrud),
            )
            // Core voxel edits need their spatial index in the same fixed tick.
            // Streaming-generated dirtiness is consumed by the variable-rate
            // Update copy before the following core tick.
            .add_systems(
                FixedUpdate,
                rebuild_virtual_chunk_index.after(apply_crud_messages),
            )
            .add_systems(
                Update,
                rebuild_virtual_chunk_index.in_set(LocalCoordinateSet::RebuildIndex),
            );
    }
}

fn apply_crud_messages(
    mut commands: Commands,
    voxels: bevy::prelude::Res<AtomicVoxelRegistry>,
    mut messages: MessageReader<LocalCoordinateCRUDMessage>,
    mut local_coordinates: Query<&mut LocalCoordinate>,
) {
    let pending_messages = messages.read();
    for message in pending_messages {
        match &message.0 {
            LocalCoordinateCRUDMessageEnum::Create { key, value } => {
                commands.entity(*key).insert(LocalCoordinate::from_voxels(
                    value
                        .iter()
                        .copied()
                        .filter(|voxel| voxel_is_registered(voxel.voxel, &voxels)),
                ));
            }
            LocalCoordinateCRUDMessageEnum::Update { key, value } => {
                if let Ok(mut local_coordinate) = local_coordinates.get_mut(*key) {
                    local_coordinate.apply_voxels(
                        value
                            .iter()
                            .copied()
                            .filter(|voxel| voxel_is_registered(voxel.voxel, &voxels)),
                    );
                }
            }
            LocalCoordinateCRUDMessageEnum::Delete { key } => {
                commands.entity(*key).remove::<LocalCoordinate>();
            }
            LocalCoordinateCRUDMessageEnum::Retrieve { .. } => {}
        }
    }
}

fn voxel_is_registered(id: crate::AtomicVoxelId, registry: &AtomicVoxelRegistry) -> bool {
    id == EMPTY_VOXEL_ID || registry.definition(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AtomicVoxelId, SOLID_VOXEL_ID};

    #[test]
    fn authoritative_crud_accepts_only_empty_or_registered_voxels() {
        let registry = AtomicVoxelRegistry::builtin();

        assert!(voxel_is_registered(EMPTY_VOXEL_ID, &registry));
        assert!(voxel_is_registered(SOLID_VOXEL_ID, &registry));
        assert!(!voxel_is_registered(AtomicVoxelId(99), &registry));
    }
}
