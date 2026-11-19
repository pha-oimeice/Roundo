//! Converts procedurally generated chunks into local-coordinate storage.

use crate::local_coordinate::data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate};
use crate::{AtomicVoxelRegistry, ChunkCoordinate};
use bevy::prelude::IVec3;
use roundo_dsa::algorithm::pcg::infinite_spheres::{
    CHUNK_EDGE_LENGTH as GENERATED_CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk,
};

/// Materializes registered nonempty voxels from a generated chunk.
pub(crate) fn replace_generated_chunk(
    local_coordinate: &mut LocalCoordinate,
    chunk: &GeneratedChunk,
    voxels: &AtomicVoxelRegistry,
) -> bool {
    let Some(chunk_position) = local_chunk_position(chunk.coordinate) else {
        return false;
    };

    // Generated array order is x-major, followed by y and z.
    let mut stored_chunk = Chunk::default();
    for (index, id) in chunk.voxels().iter().enumerate() {
        if *id == EMPTY_MATERIAL_ID {
            continue;
        }
        let x = index % GENERATED_CHUNK_EDGE_LENGTH;
        let y = index / GENERATED_CHUNK_EDGE_LENGTH % GENERATED_CHUNK_EDGE_LENGTH;
        let z = index / GENERATED_CHUNK_EDGE_LENGTH.pow(2);
        stored_chunk.set_voxel(
            IVec3::new(x as i32, y as i32, z as i32),
            voxels.generated_voxel(*id),
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
    local_chunk_position(coordinate)
        .is_some_and(|position| local_coordinate.remove_chunk(position))
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
