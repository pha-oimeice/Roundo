//! Converts procedurally generated chunks into local-coordinate storage.

use crate::local_coordinate::data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate};
use crate::{AtomicVoxelRegistry, ChunkCoordinate, GeneratedChunk};
use bevy::prelude::IVec3;

/// Materializes registered nonempty voxels from a generated chunk.
pub(crate) fn replace_generated_chunk(
    local_coordinate: &mut LocalCoordinate,
    coordinate: ChunkCoordinate,
    chunk: &GeneratedChunk,
    voxels: &AtomicVoxelRegistry,
) -> bool {
    let Some(chunk_position) = local_chunk_position(coordinate) else {
        return false;
    };

    let mut stored_chunk = Chunk::default();
    for (local, material) in chunk.nonempty_voxels() {
        stored_chunk.set_voxel(
            IVec3::from_array(local.map(|axis| axis as i32)),
            voxels.generated_voxel(material),
        );
    }
    local_coordinate.replace_chunk(chunk_position, stored_chunk);
    true
}

/// Removes a generated chunk when its coordinate fits local storage.
pub(crate) fn remove_generated_chunk(
    local_coordinate: &mut LocalCoordinate,
    coordinate: ChunkCoordinate,
) -> bool {
    local_chunk_position(coordinate).is_some_and(|position| local_coordinate.remove_chunk(position))
}

/// Converts wide generator coordinates only when every axis is representable locally.
pub(crate) fn local_chunk_position(coordinate: ChunkCoordinate) -> Option<IVec3> {
    // Bounds account for voxel-to-chunk scaling before narrowing to i32.
    let minimum = i64::from(i32::MIN / CHUNK_EDGE_LENGTH);
    let maximum = i64::from(i32::MAX / CHUNK_EDGE_LENGTH);
    if coordinate
        .iter()
        .any(|value| !(minimum..=maximum).contains(value))
    {
        return None;
    }

    Some(IVec3::new(
        coordinate[0] as i32,
        coordinate[1] as i32,
        coordinate[2] as i32,
    ))
}
