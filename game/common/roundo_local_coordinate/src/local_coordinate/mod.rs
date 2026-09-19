//! Chunked local-coordinate voxel storage and its derived collider.
//!
//! Each [`data::LocalCoordinate`] owns its chunks and fill-weighted center of
//! mass. [`VirtualChunkIndex`] is rebuilt from those chunks in absolute space on
//! both server and client. Region-loading callers query only
//! [`VirtualChunkIndex::chunks_in_radius`]; local-coordinate ownership is kept
//! behind that interface.
//!
//! Client presentation may observe compressed chunk state through the public
//! read-only interface; this module does not own meshes, materials, render
//! entities, or rendering LOD.
//! Physics privately derives one child collider per non-empty chunk.

mod base;
mod chunk;
mod client;
pub mod data;
mod derived_svo;
mod geometry;
mod pcg;
mod physics;
mod raycast;
mod server;
mod test;
#[cfg(test)]
mod tests;
mod transform;
mod virtual_chunk;
mod voxel_registry;

pub use base::LocalCoordinateSet;
pub use client::{
    ClientChunkViewDistance, DEFAULT_CHUNK_VIEW_DISTANCE, LocalCoordinateClientCommand,
    LocalCoordinateClientEvent, LocalCoordinateClientIpc, LocalCoordinateClientPlugin,
    LocalCoordinateClientWorld, MAX_CHUNK_VIEW_DISTANCE, MIN_CHUNK_VIEW_DISTANCE,
};
pub use data::{
    AtomicVoxel, AtomicVoxelId, EMPTY_VOXEL_ID, GlobalAtomicVoxelData, LocalAtomicVoxelData,
    LocalCoordinate, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
    LocalCoordinateChunkView, LocalCoordinateIdentity, PositionedAtomicVoxel, SOLID_VOXEL_ID,
};
pub use raycast::{VoxelRaycastHit, VoxelRaycaster};
pub use server::{
    DEFAULT_PCG_LOCAL_COORDINATE_ID, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc, LocalCoordinateServerPlugin, LocalCoordinateServerSet,
    LocalCoordinateServerWorld, PcgLocalCoordinate,
};
pub use transform::LocalCoordinateTransform;
pub use virtual_chunk::{
    ChunkReference, VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkIndex,
};
pub use voxel_registry::{
    AtomicVoxelRegistry, AtomicVoxelRegistryError, DEFAULT_PLACED_VOXEL_SLOT,
    GENERATED_SOLID_VOXEL_SLOT,
};

pub mod msg {
    #[allow(unused_imports)]
    pub use super::{
        data::{LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum},
        test::LocalCoordinateTestMessage,
    };
}

pub mod plugins {
    #[allow(unused_imports)]
    pub use super::{
        LocalCoordinateClientPlugin, LocalCoordinateServerPlugin, base::LocalCoordinateBasePlugin,
        physics::LocalCoordinatePhysicsPlugin, test::LocalCoordinateTestPlugin,
    };
}
