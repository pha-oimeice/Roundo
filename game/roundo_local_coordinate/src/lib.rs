#![allow(dead_code)]
pub mod local_coordinate;
pub mod roundo_physics;

pub use local_coordinate::{
    ChunkReference, DEFAULT_PCG_LOCAL_COORDINATE_ID, LocalCoordinateClientCommand,
    LocalCoordinateClientIpc, LocalCoordinateClientPlugin, LocalCoordinateClientWorld,
    LocalCoordinateServerCommand, LocalCoordinateServerEvent, LocalCoordinateServerIpc,
    LocalCoordinateServerPlugin, LocalCoordinateServerWorld, PcgLocalCoordinate,
    VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkIndex, VoxelRaycastHit,
    VoxelRaycaster,
};
pub use roundo_algorithm::pcg::infinite_spheres::{
    CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk, InfiniteSphereGenerator,
    SOLID_MATERIAL_ID, generate_chunk,
};
pub use roundo_networking::LocalCoordinateId;

pub type ChunkCoordinate = [i64; 3];
