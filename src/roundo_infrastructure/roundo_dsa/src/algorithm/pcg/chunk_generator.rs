use std::sync::Arc;

/// Fixed voxel count along each generated chunk axis.
pub const CHUNK_EDGE_LENGTH: usize = 16;
/// Material sample for empty space.
pub const EMPTY_MATERIAL_ID: u16 = 0;
/// Material sample for solid space.
pub const SOLID_MATERIAL_ID: u16 = 1;

/// Signed `[x, y, z]` position in chunk units, not voxel units.
pub type ChunkCoordinate = [i64; 3];

/// Generates chunks independently of query order and neighboring requests.
pub trait ChunkGenerator {
    /// Returns the complete chunk at `coordinate` for this generator's configuration.
    fn generate_chunk(&self, coordinate: ChunkCoordinate) -> GeneratedChunk;
}

/// Dense immutable material samples for one fixed-size chunk, without placement metadata.
/// Cloning shares the voxel storage through `Arc`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedChunk {
    /// Dense storage ordered with x fastest, followed by y and z.
    pub voxels: Arc<[u16; CHUNK_EDGE_LENGTH.pow(3)]>,
}

impl Default for GeneratedChunk {
    fn default() -> Self {
        Self {
            voxels: Arc::new([EMPTY_MATERIAL_ID; CHUNK_EDGE_LENGTH.pow(3)]),
        }
    }
}

impl GeneratedChunk {
    /// Takes dense x-major voxel storage when its length covers exactly one chunk.
    pub fn from_voxels(voxels: Vec<u16>) -> Option<Self> {
        let voxels: Box<[u16; CHUNK_EDGE_LENGTH.pow(3)]> =
            voxels.into_boxed_slice().try_into().ok()?;
        Some(Self {
            voxels: Arc::from(voxels),
        })
    }

    /// Returns a material sample at a chunk-local position.
    /// Panics if any coordinate is outside the chunk.
    pub fn voxel(&self, local_position: [usize; 3]) -> u16 {
        self.voxels[voxel_index(local_position)]
    }

    /// Yields nonempty material samples with their chunk-local voxel positions.
    pub fn nonempty_voxels(&self) -> impl Iterator<Item = ([usize; 3], u16)> + '_ {
        self.voxels
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(index, material)| {
                if material == EMPTY_MATERIAL_ID {
                    return None;
                }
                let x = index % CHUNK_EDGE_LENGTH;
                let y = index / CHUNK_EDGE_LENGTH % CHUNK_EDGE_LENGTH;
                let z = index / CHUNK_EDGE_LENGTH.pow(2);
                Some(([x, y, z], material))
            })
    }

    /// Reports whether all material samples are empty.
    pub fn is_empty(&self) -> bool {
        self.voxels
            .iter()
            .all(|material| *material == EMPTY_MATERIAL_ID)
    }
}

pub(super) fn voxel_index([x, y, z]: [usize; 3]) -> usize {
    assert!(
        x < CHUNK_EDGE_LENGTH && y < CHUNK_EDGE_LENGTH && z < CHUNK_EDGE_LENGTH,
        "chunk-local voxel position [{x}, {y}, {z}] is out of bounds"
    );
    x + y * CHUNK_EDGE_LENGTH + z * CHUNK_EDGE_LENGTH.pow(2)
}
