//! Neighbor-independent generation for a single-layer superflat world.

use super::infinite_spheres::{
    CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk, SOLID_MATERIAL_ID,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SuperflatGenerator {
    pub height: i64,
}

impl SuperflatGenerator {
    pub const fn new(height: i64) -> Self {
        Self { height }
    }

    pub fn generate_chunk(&self, x: i64, y: i64, z: i64) -> GeneratedChunk {
        generate_chunk(x, y, z, self.height)
    }
}

impl Default for SuperflatGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Pure `f(x, y, z, h) -> Chunk` generation for a solid layer at world height `h`.
pub fn generate_chunk(x: i64, y: i64, z: i64, height: i64) -> GeneratedChunk {
    let coordinate = [x, y, z];
    if y != height.div_euclid(CHUNK_EDGE_LENGTH as i64) {
        return GeneratedChunk::empty(coordinate);
    }

    let local_y = height.rem_euclid(CHUNK_EDGE_LENGTH as i64) as usize;
    let mut voxels = vec![EMPTY_MATERIAL_ID; CHUNK_EDGE_LENGTH.pow(3)];
    for local_z in 0..CHUNK_EDGE_LENGTH {
        for local_x in 0..CHUNK_EDGE_LENGTH {
            let index = local_x
                + local_y * CHUNK_EDGE_LENGTH
                + local_z * CHUNK_EDGE_LENGTH * CHUNK_EDGE_LENGTH;
            voxels[index] = SOLID_MATERIAL_ID;
        }
    }

    GeneratedChunk::from_voxels(coordinate, voxels)
        .expect("the superflat generator always emits one complete chunk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_zero_generates_exactly_one_layer() {
        let chunk = generate_chunk(4, 0, -7, 0);

        for y in 0..CHUNK_EDGE_LENGTH {
            for z in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let expected = if y == 0 {
                        SOLID_MATERIAL_ID
                    } else {
                        EMPTY_MATERIAL_ID
                    };
                    assert_eq!(chunk.voxel([x, y, z]), Some(expected));
                }
            }
        }
    }

    #[test]
    fn chunks_outside_the_layer_are_empty() {
        assert!(generate_chunk(0, -1, 0, 0).is_empty());
        assert!(generate_chunk(0, 1, 0, 0).is_empty());
    }

    #[test]
    fn negative_heights_use_euclidean_chunk_coordinates() {
        let chunk = generate_chunk(0, -1, 0, -1);

        assert_eq!(
            chunk.voxel([0, CHUNK_EDGE_LENGTH - 1, 0]),
            Some(SOLID_MATERIAL_ID)
        );
        assert_eq!(
            chunk.voxel([0, CHUNK_EDGE_LENGTH - 2, 0]),
            Some(EMPTY_MATERIAL_ID)
        );
    }
}
