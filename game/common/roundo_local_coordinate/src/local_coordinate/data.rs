use crate::local_coordinate::transform::LocalCoordinateTransform;
use bevy::prelude::{Component, Entity, IVec3, Message, Vec3};
use roundo_algorithm::tree::{BreadthFirstLosslessSvo, Octree};
use roundo_toolbox::{CRUDRequest, macros::identifier};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

/// Chunk edges are powers of two so local positions map directly to octree paths.
pub const CHUNK_EDGE_LENGTH: i32 = 16;
identifier!(AtomicVoxelId, u32);
pub type AtomicVoxel = AtomicVoxelId;

pub const EMPTY_VOXEL_ID: AtomicVoxelId = AtomicVoxelId(0);
pub const SOLID_VOXEL_ID: AtomicVoxelId = AtomicVoxelId(1);

/// Shared immutable definition for one atomic voxel identifier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GlobalAtomicVoxelData;

/// Chunk-owned persistent data associated with one atomic voxel identifier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalAtomicVoxelData;

/// Process-wide atomic voxel definitions shared by every local coordinate.
pub static GLOBAL_ATOMIC_VOXEL_DATA: LazyLock<HashMap<AtomicVoxelId, GlobalAtomicVoxelData>> =
    LazyLock::new(|| {
        HashMap::from([
            (EMPTY_VOXEL_ID, GlobalAtomicVoxelData),
            (SOLID_VOXEL_ID, GlobalAtomicVoxelData),
        ])
    });

/// A voxel change expressed in local-coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionedAtomicVoxel {
    pub position: IVec3,
    pub voxel: AtomicVoxel,
}

/// Shared CRUD input for authoritative local-coordinate voxel state.
#[derive(Message, Debug, Clone)]
pub struct LocalCoordinateCRUDMessage(pub LocalCoordinateCRUDMessageEnum);

pub type LocalCoordinateCRUDMessageEnum = CRUDRequest<Entity, Vec<PositionedAtomicVoxel>>;

/// One chunk-local triangle derived from solid voxel primitive data.
#[derive(Debug, Clone, Copy)]
pub struct VoxelTriangle {
    pub vertices: [Vec3; 3],
    pub normal: Vec3,
    pub color: [f32; 4],
}

/// A sparse fixed-size voxel region backed by an editable octree.
pub struct Chunk {
    pub octree: Octree<AtomicVoxel>,
    pub(crate) primitive_voxels: Option<Arc<[AtomicVoxel]>>,
    pub(crate) read_only_svo: Option<Arc<BreadthFirstLosslessSvo<AtomicVoxel>>>,
    pub(crate) read_only_svo_is_authoritative: bool,
    pub local_atomic_voxel_data: HashMap<AtomicVoxelId, LocalAtomicVoxelData>,
    pub triangles: Vec<VoxelTriangle>,
    pub solid_count: usize,
    pub geometry_revision: u64,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            octree: Octree::new(0, EMPTY_VOXEL_ID),
            primitive_voxels: Some(vec![EMPTY_VOXEL_ID; CHUNK_EDGE_LENGTH.pow(3) as usize].into()),
            read_only_svo: None,
            read_only_svo_is_authoritative: false,
            local_atomic_voxel_data: HashMap::new(),
            triangles: Vec::new(),
            solid_count: 0,
            geometry_revision: 0,
        }
    }
}

/// The complete voxel state and shared derived geometry for one Bevy entity.
#[derive(Component, Default)]
#[require(LocalCoordinateTransform)]
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
    /// Primitive chunk changes not yet observed by an owning runtime.
    pub(crate) changed_chunks: HashSet<IVec3>,
    /// Incremented whenever one or more chunk triangle caches change.
    pub geometry_revision: u64,
}
