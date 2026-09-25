//! Public voxel-world model, streaming contracts, and physics integration.

#![allow(dead_code)]
pub mod local_coordinate;
pub mod roundo_physics;

pub use local_coordinate::{
    AtomicVoxel, AtomicVoxelId, AtomicVoxelRegistry, AtomicVoxelRegistryError, ChunkReference,
    ClientChunkViewDistance, CompleteEnvironmentDefaults, DEFAULT_CHUNK_VIEW_DISTANCE,
    DEFAULT_PCG_LOCAL_COORDINATE_ID, DEFAULT_PLACED_VOXEL_SLOT, EMPTY_VOXEL_ID,
    GENERATED_SOLID_VOXEL_SLOT, GlobalAtomicVoxelData, LocalAtomicVoxelData, LocalCoordinate,
    LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum, LocalCoordinateChunkView,
    LocalCoordinateClientCommand, LocalCoordinateClientEvent, LocalCoordinateClientIpc,
    LocalCoordinateClientPlugin, LocalCoordinateClientWorld, LocalCoordinateIdentity,
    LocalCoordinatePhysicsInterests, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc, LocalCoordinateServerPlugin, LocalCoordinateServerSet,
    LocalCoordinateServerWorld, LocalCoordinateSet, LocalCoordinateTransform,
    MAX_CHUNK_VIEW_DISTANCE, MIN_CHUNK_VIEW_DISTANCE, PcgLocalCoordinate, PositionedAtomicVoxel,
    SOLID_VOXEL_ID, SparseEnvironmentOverride, VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate,
    VirtualChunkEnvironmentMap, VirtualChunkIndex, VoxelRaycastHit, VoxelRaycaster,
    virtual_chunk_coordinate_for_position,
};
pub use roundo_algorithm::pcg::{
    infinite_spheres::{CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk, SOLID_MATERIAL_ID},
    superflat::{SuperflatGenerator, generate_chunk},
};
pub use roundo_contracts::{ChunkId, ChunkVersion, LocalCoordinateId};
/// Lossless sparse representation exchanged for a voxel chunk.
pub type VoxelChunkSvo = roundo_algorithm::tree::BreadthFirstLosslessSvo<AtomicVoxel>;

/// Signed chunk position in local-coordinate space.
pub type ChunkCoordinate = [i64; 3];
