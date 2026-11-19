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

/// Loads authoritative voxel mutation and shared coordinate derivation.
pub struct LocalCoordinateBasePlugin;

/// Installs read-side coordinate derivation without authoritative CRUD filtering.
struct LocalCoordinateReadPlugin;

/// Presents voxel data produced in the same World, without a separate chunk store.
pub struct LocalCoordinatePresentationPlugin;

impl Plugin for LocalCoordinatePresentationPlugin {
    fn build(&self, app: &mut App) {
        use super::physics::{LocalCoordinatePhysicsPlugin, LocalCoordinatePhysicsSet};
        app.add_plugins((LocalCoordinateBasePlugin, LocalCoordinatePhysicsPlugin))
            .add_systems(
                Update,
                (cache_render_chunks, clear_chunk_changes)
                    .chain()
                    .after(LocalCoordinateSet::RebuildIndex)
                    .after(LocalCoordinatePhysicsSet::Sync),
            );
    }
}

fn cache_render_chunks(mut coordinates: Query<&mut LocalCoordinate>) {
    for mut coordinate in &mut coordinates {
        if coordinate.changed_chunks.is_empty() {
            continue;
        }
        let dirty = coordinate
            .changed_chunks
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for position in dirty {
            if let Some(chunk) = coordinate.chunks.get_mut(&position) {
                chunk.read_only_svo();
            }
        }
    }
}

pub(super) fn clear_chunk_changes(mut coordinates: Query<&mut LocalCoordinate>) {
    for mut coordinate in &mut coordinates {
        if !coordinate.changed_chunks.is_empty() {
            coordinate.changed_chunks.clear();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub enum LocalCoordinateSet {
    ApplyCrud,
    RebuildIndex,
}

impl Plugin for LocalCoordinateBasePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LocalCoordinateReadPlugin)
            .add_message::<LocalCoordinateCRUDMessage>()
            .add_systems(
                FixedUpdate,
                apply_crud_messages.in_set(LocalCoordinateSet::ApplyCrud),
            );
    }
}

impl Plugin for LocalCoordinateReadPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransformPlugin>() {
            app.add_plugins(TransformPlugin);
        }
        configure_transform_derivation(app);
        app.init_resource::<AtomicVoxelRegistry>()
            .init_resource::<VirtualChunkIndex>()
            // Core voxel edits need their spatial index in the same fixed tick.
            // Procedural generation dirtiness is consumed by the variable-rate
            // Update copy before the following core tick.
            .add_systems(
                FixedUpdate,
                rebuild_virtual_chunk_index
                    .in_set(LocalCoordinateSet::RebuildIndex)
                    .after(LocalCoordinateSet::ApplyCrud),
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
