//! Procedural generation algorithms independent of ECS and storage crates.
pub mod chunk_generator;
pub mod infinite_spheres;
pub mod maze;
pub mod superflat;

pub use chunk_generator::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkGenerator, EMPTY_MATERIAL_ID, GeneratedChunk,
    SOLID_MATERIAL_ID,
};
