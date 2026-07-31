use bevy::prelude::{Component, Entity, IVec3, Message, Vec3};
use roundo_algorithm::tree::Octree;
use roundo_toolbox::CRUDRequest;
use std::collections::{HashMap, HashSet};

/// Chunk edges are powers of two so local positions map directly to octree paths.
pub const CHUNK_EDGE_LENGTH: i32 = 16;
pub const EMPTY_VOXEL_ID: u32 = 0;
pub const SOLID_VOXEL_ID: u32 = 1;

/// A voxel change expressed in local-coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicVoxel {
    pub position: IVec3,
    pub data: AtomicVoxelData,
}

/// Authoritative per-voxel state. `0` is air and `1` is solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicVoxelData {
    pub id: u32,
}

impl AtomicVoxelData {
    pub fn is_solid(self) -> bool {
        self.id == SOLID_VOXEL_ID
    }
}

/// Shared CRUD input for authoritative local-coordinate voxel state.
#[derive(Message, Debug, Clone)]
pub struct LocalCoordinateCRUDMessage(pub LocalCoordinateCRUDMessageEnum);

pub type LocalCoordinateCRUDMessageEnum = CRUDRequest<Entity, Vec<AtomicVoxel>>;

/// One chunk-local triangle derived from solid voxel primitive data.
#[derive(Debug, Clone, Copy)]
pub struct VoxelTriangle {
    pub vertices: [Vec3; 3],
    pub normal: Vec3,
    pub color: [f32; 4],
}

/// A sparse fixed-size voxel region backed by an editable octree.
pub struct Chunk {
    pub octree: Octree<Option<AtomicVoxelData>>,
    pub triangles: Vec<VoxelTriangle>,
    pub solid_count: usize,
    pub geometry_revision: u64,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            octree: Octree::new(0, None),
            triangles: Vec::new(),
            solid_count: 0,
            geometry_revision: 0,
        }
    }
}

/// The complete voxel state and shared derived geometry for one Bevy entity.
#[derive(Component, Default)]
pub struct LocalCoordinate {
    /// Loaded primitive data partitioned by chunk coordinate, including empty chunks.
    pub chunks: HashMap<IVec3, Chunk>,
    /// Fill-weighted center of the owned chunks in local-coordinate space.
    pub center_of_mass: Vec3,
    /// Stable per-voxel RGB values used while materializing chunk triangles.
    pub voxel_colors: HashMap<IVec3, [f32; 4]>,
    /// Monotonic identifier backing collision-free RGB allocation.
    pub next_color: u128,
    /// Chunks whose triangles must be regenerated before the next output update.
    pub dirty_chunks: HashSet<IVec3>,
    /// Incremented whenever one or more chunk triangle caches change.
    pub geometry_revision: u64,
}
