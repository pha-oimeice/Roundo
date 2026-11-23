//! Public voxel-world model, procedural generation, physics, and rendering.

#![allow(dead_code)]
pub mod local_coordinate;
pub mod roundo_physics;
mod voxel_render;

pub use voxel_render::{WorldRenderPlugin, WorldRenderSettings};

/// Native attribute definitions; existing root type exports remain available.
pub use local_coordinate::attributes;

pub use local_coordinate::{
    AtomicVoxel, AtomicVoxelId, AtomicVoxelRegistry, AtomicVoxelRegistryError, ChunkReference,
    CompleteEnvironmentDefaults, DEFAULT_PCG_LOCAL_COORDINATE_ID, DEFAULT_PLACED_VOXEL_SLOT,
    EMPTY_VOXEL_ID, GENERATED_SOLID_VOXEL_SLOT, GlobalAtomicVoxelData, LocalAtomicVoxelData,
    LocalCoordinate, LocalCoordinateCRUDMessage, LocalCoordinateCRUDMessageEnum,
    LocalCoordinateChunkView, LocalCoordinateIdentity, LocalCoordinatePhysicsInterests,
    LocalCoordinateServerPlugin, LocalCoordinateServerSet, LocalCoordinateServerWorld,
    LocalCoordinateSet, LocalCoordinateTransform, PcgLocalCoordinate, PhysicsDemandBudget,
    PhysicsDemandRegion, PositionedAtomicVoxel, SOLID_VOXEL_ID, SparseEnvironmentOverride,
    VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkEnvironmentMap,
    VirtualChunkIndex, VoxelEditBudget, VoxelRaycastHit, VoxelRaycaster,
    WorldInteractionBudgetError, virtual_chunk_coordinate_for_position,
};
pub use local_coordinate::{
    CoordinateIncarnation, LocalPhysicsDemandFacts, LocalPhysicsDemands, PhysicsDemandId,
    PhysicsRegionReadiness, WorldInteractionError, WorldInteractionPlugin,
    WorldInteractionRuntimeBudgets,
};
pub use local_coordinate::{
    LocalVoxelEdits, VoxelEditCompletion, VoxelEditError, VoxelEditFacts, VoxelEditId,
    VoxelEditOutcome, VoxelEditRejection, VoxelEditRequest, VoxelEditSet, VoxelEditTarget,
};
pub use roundo_contracts::{ChunkId, LocalCoordinateId};
pub use roundo_infrastructure::dsa::algorithm::pcg::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkGenerator, EMPTY_MATERIAL_ID, GeneratedChunk,
    SOLID_MATERIAL_ID, superflat::SuperflatGenerator,
};
/// Lossless sparse representation used to render a voxel chunk.
pub type VoxelChunkSvo =
    roundo_infrastructure::dsa::data_structure::tree::BreadthFirstLosslessSvo<AtomicVoxel>;
