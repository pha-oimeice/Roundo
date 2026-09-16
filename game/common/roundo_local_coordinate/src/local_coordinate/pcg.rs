//! Converts generated or transferred chunks into local-coordinate storage.

use crate::local_coordinate::data::{CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate};
use crate::{AtomicVoxelRegistry, ChunkCoordinate, VoxelChunkSvo};
use bevy::prelude::IVec3;
use roundo_algorithm::pcg::infinite_spheres::{
    CHUNK_EDGE_LENGTH as GENERATED_CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk,
};
use std::sync::Arc;

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

/// Installs an immutable transferred SVO without primitive expansion.
pub(crate) fn replace_read_only_chunk(
    local_coordinate: &mut LocalCoordinate,
    coordinate: ChunkCoordinate,
    view: Arc<VoxelChunkSvo>,
) -> bool {
    let Some(chunk_position) = local_chunk_position(coordinate) else {
        return false;
    };
    local_coordinate.replace_chunk(
        chunk_position,
        crate::local_coordinate::data::Chunk::from_read_only_svo(view),
    );
    true
}

/// Removes a generated chunk when its coordinate fits local storage.
pub(crate) fn remove_generated_chunk(
    local_coordinate: &mut LocalCoordinate,
    coordinate: ChunkCoordinate,
) -> bool {
    local_chunk_position(coordinate).is_some_and(|position| local_coordinate.remove_chunk(position))
}

/// Converts wire coordinates only when every axis is representable locally.
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

#[cfg(test)]
mod tests {
    use super::*;
    use roundo_algorithm::pcg::infinite_spheres::SOLID_MATERIAL_ID;

    #[test]
    fn generated_chunk_voxels_are_stored_in_local_coordinate_space() {
        let mut voxels = vec![EMPTY_MATERIAL_ID; GENERATED_CHUNK_EDGE_LENGTH.pow(3)];
        let local_position = [1, 2, 3];
        let index = local_position[0]
            + local_position[1] * GENERATED_CHUNK_EDGE_LENGTH
            + local_position[2] * GENERATED_CHUNK_EDGE_LENGTH.pow(2);
        voxels[index] = SOLID_MATERIAL_ID;
        let chunk = GeneratedChunk::from_voxels([2, -1, 0], voxels)
            .expect("test chunk has the required volume");
        let mut local_coordinate = LocalCoordinate::default();

        assert!(replace_generated_chunk(
            &mut local_coordinate,
            &chunk,
            &AtomicVoxelRegistry::builtin()
        ));
        assert!(local_coordinate.chunks[&IVec3::new(2, -1, 0)].is_solid(IVec3::new(1, 2, 3)));
        assert!(remove_generated_chunk(
            &mut local_coordinate,
            chunk.coordinate
        ));
        assert!(local_coordinate.chunks.is_empty());
    }

    #[test]
    fn empty_generated_chunk_is_retained_as_loaded_data() {
        let chunk = GeneratedChunk::empty([3, 4, 5]);
        let mut local_coordinate = LocalCoordinate::default();

        assert!(replace_generated_chunk(
            &mut local_coordinate,
            &chunk,
            &AtomicVoxelRegistry::builtin()
        ));
        assert!(local_coordinate.chunks.contains_key(&IVec3::new(3, 4, 5)));
        assert!(local_coordinate.chunks[&IVec3::new(3, 4, 5)].is_empty());
    }
}
