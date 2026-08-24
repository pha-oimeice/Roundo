mod error;
mod octree;
mod optimized;
mod unoptimized;

pub use error::{OctreeError, OptimizedOctreeError};
pub use octree::Octree;
pub use optimized::{
    CompressionSettings, CompressionThresholdContext, CompressionThresholdSchedule,
    DensityAggregation, DensityAggregationContext, LosslessSvo, LosslessSvoNode, NO_CHILDREN,
    OctreeDensity, OptimizedNode, OptimizedOctree, ReadOnlyBuffer,
};
pub use unoptimized::{Node, UnoptimizedOctree};

#[cfg(test)]
mod tests;
