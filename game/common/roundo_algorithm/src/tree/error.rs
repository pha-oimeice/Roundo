//! Structural and encoding failures exposed by octree implementations.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Conditions that prevent lossless compact SVO construction.
pub enum OptimizedOctreeError {
    NodeCountExceedsIndexRange,
    MaximumDepthExceedsCoordinateBits,
    SourceExceedsMaximumDepth,
}

impl fmt::Display for OptimizedOctreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeCountExceedsIndexRange => {
                write!(formatter, "optimized octree exceeds the u32 index range")
            }
            Self::MaximumDepthExceedsCoordinateBits => {
                write!(formatter, "SVO depth exceeds the u32 coordinate range")
            }
            Self::SourceExceedsMaximumDepth => {
                write!(
                    formatter,
                    "source octree contains nodes below the requested SVO depth"
                )
            }
        }
    }
}

impl std::error::Error for OptimizedOctreeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Invalid mutations against the editable octree representation.
pub enum OctreeError {
    ChildIndexOutOfBounds { index: u32 },
    NodeNotFound { id: u32 },
    DuplicateNodeId { id: u32 },
    ChildSlotOccupied { parent_id: u32, child_index: u32 },
    CannotRemoveRoot,
}

impl fmt::Display for OctreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChildIndexOutOfBounds { index } => {
                write!(formatter, "child index {index} is outside the octree range")
            }
            Self::NodeNotFound { id } => write!(formatter, "node {id} was not found"),
            Self::DuplicateNodeId { id } => write!(formatter, "node ID {id} already exists"),
            Self::ChildSlotOccupied {
                parent_id,
                child_index,
            } => write!(
                formatter,
                "child slot {child_index} of node {parent_id} is already occupied"
            ),
            Self::CannotRemoveRoot => write!(formatter, "the octree root cannot be removed"),
        }
    }
}

// Both enums retain copy semantics so failed mutations remain inexpensive.
impl std::error::Error for OctreeError {}
