mod client;
mod derivation;
mod server;
mod svo;

pub use client::{
    ClientStaticVoxelChunk, StaticVoxelChunk, StaticVoxelClientCommand, StaticVoxelClientIpc,
    StaticVoxelClientPlugin, StaticVoxelClientWorld,
};
pub use roundo_algorithm::pcg::infinite_spheres::{
    CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk, InfiniteSphereGenerator,
    SOLID_MATERIAL_ID, generate_chunk,
};
pub use server::{
    CHARACTER_CHUNK_LOAD_RADIUS, StaticVoxelServerCommand, StaticVoxelServerEvent,
    StaticVoxelServerIpc, StaticVoxelServerPlugin, StaticVoxelServerWorld,
};
pub use svo::StaticVoxelSvoView;

pub type ChunkCoordinate = [i64; 3];
