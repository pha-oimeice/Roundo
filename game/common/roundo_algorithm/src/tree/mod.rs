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
    ReadOnlyBuffer,
};
pub use unoptimized::{Node, UnoptimizedOctree};

#[cfg(test)]
mod tests;
