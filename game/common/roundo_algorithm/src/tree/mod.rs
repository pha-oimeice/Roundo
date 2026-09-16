//! Octree representations and lossless sparse-voxel compression.

mod error;
mod octree;
mod optimized;
mod unoptimized;

pub use error::{OctreeError, OptimizedOctreeError};
pub use octree::Octree;
pub use optimized::{
    BreadthFirstLosslessSvo, BreadthFirstLosslessSvoNode, CompressionSettings,
    CompressionThresholdContext, CompressionThresholdSchedule, DensityAggregation,
    DensityAggregationContext, NO_CHILDREN, OctreeDensity, OptimizedNode, OptimizedOctree,
    PackedSvo, PackedSvoNode, ReadOnlyBuffer,
};
/// Mutable reference representation used before compact encoding.
pub use unoptimized::{Node, UnoptimizedOctree};

#[cfg(test)]
mod tests;
