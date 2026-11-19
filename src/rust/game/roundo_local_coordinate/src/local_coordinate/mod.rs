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

pub mod attributes;
mod base;
mod chunk;
pub mod data;
mod environment;
mod geometry;
mod pcg;
mod physics;
mod raycast;
mod server;
mod test;
mod transform;
mod virtual_chunk;
mod voxel_edit;
mod voxel_registry;
pub use voxel_edit::{
    LocalVoxelEdits, VoxelEditCompletion, VoxelEditError, VoxelEditFacts, VoxelEditId,
    VoxelEditOutcome, VoxelEditRejection, VoxelEditRequest, VoxelEditSet, VoxelEditTarget,
};
mod world_interaction;
mod world_interaction_budget;

pub use world_interaction::{
    CoordinateIncarnation, LocalPhysicsDemandFacts, LocalPhysicsDemands, PhysicsDemandId,
    PhysicsRegionReadiness, WorldInteractionError, WorldInteractionPlugin,
    WorldInteractionRuntimeBudgets,
};

pub use attributes::{
    CompleteEnvironmentDefaults, LocalCoordinateTransform, SparseEnvironmentOverride,
};
pub use base::LocalCoordinateSet;
pub use data::{
    AtomicVoxel, AtomicVoxelId, EMPTY_VOXEL_ID, GlobalAtomicVoxelData, LocalAtomicVoxelData,
    LocalCoordinate, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
    LocalCoordinateChunkView, LocalCoordinateIdentity, PositionedAtomicVoxel, SOLID_VOXEL_ID,
};
pub use environment::VirtualChunkEnvironmentMap;
pub use physics::LocalCoordinatePhysicsInterests;
pub use raycast::{VoxelRaycastHit, VoxelRaycaster};
pub use server::{
    DEFAULT_PCG_LOCAL_COORDINATE_ID, LocalCoordinateServerPlugin, LocalCoordinateServerSet,
    LocalCoordinateServerWorld, PcgLocalCoordinate,
};
pub use virtual_chunk::{
    ChunkReference, VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkIndex,
    virtual_chunk_coordinate_for_position,
};
pub use voxel_registry::{
    AtomicVoxelRegistry, AtomicVoxelRegistryError, DEFAULT_PLACED_VOXEL_SLOT,
    GENERATED_SOLID_VOXEL_SLOT,
};

pub use world_interaction_budget::{
    PhysicsDemandBudget, PhysicsDemandRegion, VoxelEditBudget, WorldInteractionBudgetError,
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
        LocalCoordinateServerPlugin,
        base::{LocalCoordinateBasePlugin, LocalCoordinatePresentationPlugin},
        physics::LocalCoordinatePhysicsPlugin,
        test::LocalCoordinateTestPlugin,
    };
}
