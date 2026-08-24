#![allow(dead_code)]
pub mod local_coordinate;
pub mod roundo_physics;

pub use local_coordinate::{
    AtomicVoxel, AtomicVoxelId, ChunkReference, DEFAULT_PCG_LOCAL_COORDINATE_ID, EMPTY_VOXEL_ID,
    GLOBAL_ATOMIC_VOXEL_DATA, GlobalAtomicVoxelData, LocalAtomicVoxelData, LocalCoordinate,
    LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum, LocalCoordinateClientCommand,
    LocalCoordinateClientEvent, LocalCoordinateClientIpc, LocalCoordinateClientPlugin,
    LocalCoordinateClientWorld, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc, LocalCoordinateServerPlugin, LocalCoordinateServerWorld,
    LocalCoordinateSet, LocalCoordinateTransform, PcgLocalCoordinate, PositionedAtomicVoxel,
    SOLID_VOXEL_ID, VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkIndex,
    VoxelRaycastHit, VoxelRaycaster,
};
pub use roundo_algorithm::pcg::{
    infinite_spheres::{CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk, SOLID_MATERIAL_ID},
    superflat::{SuperflatGenerator, generate_chunk},
};
pub use roundo_networking::{ChunkId, ChunkVersion, LocalCoordinateId};
pub type VoxelChunkSvo = roundo_algorithm::tree::LosslessSvo<AtomicVoxel>;

pub type ChunkCoordinate = [i64; 3];
