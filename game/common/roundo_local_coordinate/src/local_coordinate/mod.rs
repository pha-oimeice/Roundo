//! Chunked local-coordinate voxel storage and its derived mesh and collider.
//!
//! Each [`data::LocalCoordinate`] owns its chunks and fill-weighted center of
//! mass. [`VirtualChunkIndex`] is rebuilt from those chunks in absolute space on
//! both server and client. Region-loading callers query only
//! [`VirtualChunkIndex::chunks_in_radius`]; local-coordinate ownership is kept
//! behind that interface.
//!
//! Derived triangles remain chunk-local. Rendering materializes one render
//! object per chunk, while physics materializes one child collider per non-empty
//! chunk; the local coordinate alone owns their shared transform and rigid body.

mod base;
mod chunk;
mod client;
pub mod data;
mod derived_svo;
mod geometry;
mod mesh;
mod pcg;
mod physics;
mod raycast;
mod server;
mod test;
#[cfg(test)]
mod tests;
mod transform;
mod virtual_chunk;

pub use base::LocalCoordinateSet;
pub use client::{
    LocalCoordinateClientCommand, LocalCoordinateClientEvent, LocalCoordinateClientIpc,
    LocalCoordinateClientPlugin, LocalCoordinateClientWorld,
};
pub use data::{
    AtomicVoxel, AtomicVoxelId, EMPTY_VOXEL_ID, GLOBAL_ATOMIC_VOXEL_DATA, GlobalAtomicVoxelData,
    LocalAtomicVoxelData, LocalCoordinate, LocalCoordinateCRUDMessage,
    LocalCoordinateCRUDMessageEnum, PositionedAtomicVoxel, SOLID_VOXEL_ID,
};
pub use raycast::{VoxelRaycastHit, VoxelRaycaster};
pub use server::{
    DEFAULT_PCG_LOCAL_COORDINATE_ID, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc, LocalCoordinateServerPlugin, LocalCoordinateServerWorld,
    PcgLocalCoordinate,
};
pub use transform::LocalCoordinateTransform;
pub use virtual_chunk::{
    ChunkReference, VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkIndex,
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
        mesh::LocalCoordinateMeshPlugin, physics::LocalCoordinatePhysicsPlugin,
        test::LocalCoordinateTestPlugin,
    };
}
