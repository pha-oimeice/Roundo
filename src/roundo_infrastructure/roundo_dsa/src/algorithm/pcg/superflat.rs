//! Neighbor-independent generation for a single-layer superflat world.

use super::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkGenerator, EMPTY_MATERIAL_ID, GeneratedChunk,
    SOLID_MATERIAL_ID,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Generator configured by an inclusive solid-world height.
pub struct SuperflatGenerator {
    pub height: i64,
}

impl SuperflatGenerator {
    /// Creates a generator with the requested surface height.
    pub const fn new(height: i64) -> Self {
        Self { height }
    }
}

impl Default for SuperflatGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ChunkGenerator for SuperflatGenerator {
    fn generate_chunk(&self, [_, y, _]: ChunkCoordinate) -> GeneratedChunk {
        if y != self.height.div_euclid(CHUNK_EDGE_LENGTH as i64) {
            return GeneratedChunk::default();
        }

        let local_y = self.height.rem_euclid(CHUNK_EDGE_LENGTH as i64) as usize;
        let mut voxels = vec![EMPTY_MATERIAL_ID; CHUNK_EDGE_LENGTH.pow(3)];
        for local_z in 0..CHUNK_EDGE_LENGTH {
            let start = (local_y + local_z * CHUNK_EDGE_LENGTH) * CHUNK_EDGE_LENGTH;
            voxels[start..start + CHUNK_EDGE_LENGTH].fill(SOLID_MATERIAL_ID);
        }

        GeneratedChunk::from_voxels(voxels)
            .expect("the superflat generator always emits one complete chunk")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_zero_generates_exactly_one_layer() {
        let chunk = SuperflatGenerator::new(0).generate_chunk([4, 0, -7]);

        for y in 0..CHUNK_EDGE_LENGTH {
            for z in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let expected = if y == 0 {
                        SOLID_MATERIAL_ID
                    } else {
                        EMPTY_MATERIAL_ID
                    };
                    assert_eq!(chunk.voxel([x, y, z]), expected);
                }
            }
        }
    }
}
