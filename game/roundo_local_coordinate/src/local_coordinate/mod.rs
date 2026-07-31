//! Chunked local-coordinate voxel storage and its derived mesh and collider.
//!
//! Each [`data::LocalCoordinate`] owns its chunks and fill-weighted center of
//! mass. [`VirtualChunkIndex`] is rebuilt from those chunks in absolute space on
//! both server and client. Region-loading callers query only
//! [`VirtualChunkIndex::chunks_in_radius`]; local-coordinate ownership is kept
//! behind that interface.

mod base;
mod chunk;
mod client;
pub mod data;
mod geometry;
mod pcg;
mod physics;
mod raycast;
mod render;
mod server;
mod test;
#[cfg(test)]
mod tests;
mod virtual_chunk;

pub use client::{
    LocalCoordinateClientCommand, LocalCoordinateClientIpc, LocalCoordinateClientPlugin,
    LocalCoordinateClientWorld,
};
pub use raycast::{VoxelRaycastHit, VoxelRaycaster};
pub use server::{
    DEFAULT_PCG_LOCAL_COORDINATE_ID, LocalCoordinateServerCommand, LocalCoordinateServerEvent,
    LocalCoordinateServerIpc, LocalCoordinateServerPlugin, LocalCoordinateServerWorld,
    PcgLocalCoordinate,
};
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
        physics::LocalCoordinatePhysicsPlugin, render::LocalCoordinateRenderPlugin,
        test::LocalCoordinateTestPlugin,
    };
}
