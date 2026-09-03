use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, ops::Deref};

use super::{
    error::OptimizedOctreeError,
    unoptimized::{Node, UnoptimizedOctree},
};

/// Sentinel used by compact nodes that have no children.
pub const NO_CHILDREN: u32 = u32::MAX;

/// One node in a lossless SVO stored in breadth-first order.
///
/// `data` is the value inherited by every missing child region. A node with no
/// children therefore represents one uniform region at its current depth.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BreadthFirstLosslessSvoNode<T> {
    pub first_child: u32,
    pub child_mask: u8,
    pub data: T,
}

/// Immutable, breadth-first SVO preserving the value at every finest-level coordinate.
///
/// Source topology is intentionally not preserved: uniform regions are collapsed and
/// children equal to their parent's inherited value are omitted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BreadthFirstLosslessSvo<T> {
    nodes: Box<[BreadthFirstLosslessSvoNode<T>]>,
    root_index: u32,
    maximum_depth: u8,
}

impl<T: Clone + Eq> BreadthFirstLosslessSvo<T> {
    /// Builds a canonical voxel-field view from a sparse editable octree.
    ///
    /// Missing source children inherit their parent's value. Explicit uniform
    /// children are collapsed when all finest-level coordinate queries remain equal.
    pub fn from_unoptimized_mapped<Source, Map>(
        source: &UnoptimizedOctree<Source>,
        maximum_depth: u8,
        map_data: Map,
    ) -> Result<Self, OptimizedOctreeError>
    where
        Map: Fn(&Source) -> T,
    {
        if maximum_depth > u32::BITS as u8 {
            return Err(OptimizedOctreeError::MaximumDepthExceedsCoordinateBits);
        }

        let root = LosslessRegion::from_source(&source.root, 0, maximum_depth, &map_data)?;
        let mut pending_nodes = VecDeque::from([&root]);
        let mut nodes = Vec::new();

        while let Some(source_node) = pending_nodes.pop_front() {
            let mut child_mask = 0_u8;
            for (octant, child) in source_node.children.iter().enumerate() {
                if child.is_some() {
                    child_mask |= 1_u8 << octant;
                }
            }
            let first_child = if child_mask == 0 {
                NO_CHILDREN
            } else {
                let index = nodes
                    .len()
                    .checked_add(pending_nodes.len())
                    .and_then(|index| index.checked_add(1))
                    .ok_or(OptimizedOctreeError::NodeCountExceedsIndexRange)?;
                compact_index(index)?
            };
            nodes.push(BreadthFirstLosslessSvoNode {
                first_child,
                child_mask,
                data: source_node.data.clone(),
            });
            pending_nodes.extend(source_node.children.iter().flatten().map(Box::as_ref));
        }

        Ok(Self {
            nodes: nodes.into_boxed_slice(),
            root_index: 0,
            maximum_depth,
        })
    }
}

impl<T> BreadthFirstLosslessSvo<T> {
    pub fn nodes(&self) -> &[BreadthFirstLosslessSvoNode<T>] {
        &self.nodes
    }

    pub const fn root_index(&self) -> u32 {
        self.root_index
    }

    pub const fn maximum_depth(&self) -> u8 {
        self.maximum_depth
    }

    pub fn child_index(&self, node_index: u32, octant: u8) -> Option<u32> {
        if octant >= 8 {
            return None;
        }
        let node = self.nodes.get(usize::try_from(node_index).ok()?)?;
        let octant_bit = 1_u8 << octant;
        if node.child_mask & octant_bit == 0 {
            return None;
        }
        let preceding_children = (node.child_mask & (octant_bit - 1)).count_ones();
        let child_index = node.first_child.checked_add(preceding_children)?;
        self.nodes
            .get(usize::try_from(child_index).ok()?)
            .map(|_| child_index)
    }

    /// Returns an explicitly stored node. Missing inherited regions return `None`.
    pub fn node_at_path(&self, path: &[u8]) -> Option<&BreadthFirstLosslessSvoNode<T>> {
        if path.len() > usize::from(self.maximum_depth) {
            return None;
        }
        let mut node_index = self.root_index;
        for &octant in path {
            node_index = self.child_index(node_index, octant)?;
        }
        self.nodes.get(usize::try_from(node_index).ok()?)
    }

    /// Returns the value of a spatial path, including values inherited from a
    /// uniform ancestor or a missing child region.
    pub fn value_at_path(&self, path: &[u8]) -> Option<&T> {
        if path.len() > usize::from(self.maximum_depth) {
            return None;
        }
        let mut node_index = self.root_index;
        for &octant in path {
            if octant >= 8 {
                return None;
            }
            let Some(child_index) = self.child_index(node_index, octant) else {
                return self
                    .nodes
                    .get(usize::try_from(node_index).ok()?)
                    .map(|node| &node.data);
            };
            node_index = child_index;
        }
        self.nodes
            .get(usize::try_from(node_index).ok()?)
            .map(|node| &node.data)
    }

    /// Queries a finest-level coordinate without allocating an octant path.
    pub fn value_at_coordinates(&self, coordinates: [u32; 3]) -> Option<&T> {
        if self.maximum_depth < u32::BITS as u8 {
            let edge = 1_u32 << self.maximum_depth;
            if coordinates.iter().any(|coordinate| *coordinate >= edge) {
                return None;
            }
        }

        let mut node_index = self.root_index;
        for depth in 0..self.maximum_depth {
            let bit = self.maximum_depth - depth - 1;
            let octant = ((coordinates[0] >> bit) & 1)
                | (((coordinates[1] >> bit) & 1) << 1)
                | (((coordinates[2] >> bit) & 1) << 2);
            let Some(child_index) = self.child_index(node_index, octant as u8) else {
                return self
                    .nodes
                    .get(usize::try_from(node_index).ok()?)
                    .map(|node| &node.data);
            };
            node_index = child_index;
        }
        self.nodes
            .get(usize::try_from(node_index).ok()?)
            .map(|node| &node.data)
    }
}

struct LosslessRegion<T> {
    data: T,
    children: [Option<Box<LosslessRegion<T>>>; 8],
}

impl<T: Clone + Eq> LosslessRegion<T> {
    fn from_source<Source, Map>(
        source: &Node<Source, 8>,
        depth: u8,
        maximum_depth: u8,
        map_data: &Map,
    ) -> Result<Self, OptimizedOctreeError>
    where
        Map: Fn(&Source) -> T,
    {
        let inherited_data = map_data(&source.data);
        if depth == maximum_depth {
            if source.children.iter().any(Option::is_some) {
                return Err(OptimizedOctreeError::SourceExceedsMaximumDepth);
            }
            return Ok(Self::uniform(inherited_data));
        }

        let mut child_regions = Vec::with_capacity(8);
        for child in &source.children {
            child_regions.push(match child.as_deref() {
                Some(child) => Self::from_source(child, depth + 1, maximum_depth, map_data)?,
                None => Self::uniform(inherited_data.clone()),
            });
        }

        if child_regions
            .iter()
            .all(|child| child.is_uniform_with(&child_regions[0].data))
        {
            return Ok(Self::uniform(child_regions[0].data.clone()));
        }

        let children = match child_regions
            .into_iter()
            .map(|child| (!child.is_uniform_with(&inherited_data)).then(|| Box::new(child)))
            .collect::<Vec<_>>()
            .try_into()
        {
            Ok(children) => children,
            Err(_) => unreachable!("one octree node always has eight child regions"),
        };
        Ok(Self {
            data: inherited_data,
            children,
        })
    }

    fn uniform(data: T) -> Self {
        Self {
            data,
            children: std::array::from_fn(|_| None),
        }
    }

    fn is_uniform_with(&self, data: &T) -> bool {
        self.children.iter().all(Option::is_none) && self.data == *data
    }
}

/// Supplies the occupancy density of a leaf value for solid compression.
pub trait OctreeDensity {
    fn density(&self) -> f32;
}

impl OctreeDensity for () {
    fn density(&self) -> f32 {
        1.0
    }
}

impl<T> OctreeDensity for Option<T> {
    fn density(&self) -> f32 {
        if self.is_some() { 1.0 } else { 0.0 }
    }
}

/// How an internal node derives density from its eight octants.
#[derive(Clone, Copy)]
pub enum DensityAggregation {
    /// Preserves the accumulated density mass of all present descendants.
    Accumulated,
    /// Normalizes density by averaging all eight octants, including empty octants.
    Average,
    /// Lets applications define their own internal density inheritance rule.
    Custom(fn(DensityAggregationContext) -> f32),
}

impl DensityAggregation {
    fn aggregate(self, context: DensityAggregationContext) -> f32 {
        let density = match self {
            Self::Accumulated => context.child_densities.iter().sum(),
            Self::Average => context.child_densities.iter().sum::<f32>() / 8.0,
            Self::Custom(aggregate) => aggregate(context),
        };

        sanitize_density(density)
    }
}

/// Inputs available to a custom [`DensityAggregation`] function.
#[derive(Clone, Copy)]
pub struct DensityAggregationContext {
    pub depth_from_root: usize,
    pub levels_below: usize,
    pub child_densities: [f32; 8],
}

/// How the solid-compression threshold changes as a node represents more space.
#[derive(Clone, Copy)]
pub enum CompressionThresholdSchedule {
    /// Uses the base threshold at every octree depth.
    Constant,
    /// Multiplies the base threshold once for each level below the node.
    Exponential { multiplier_per_level: f32 },
    /// Lets applications set an absolute threshold from node compression context.
    Custom(fn(CompressionThresholdContext) -> f32),
}

impl CompressionThresholdSchedule {
    fn threshold(
        self,
        base_solid_compression_threshold: f32,
        context: CompressionThresholdContext,
    ) -> f32 {
        match self {
            Self::Constant => base_solid_compression_threshold,
            Self::Exponential {
                multiplier_per_level,
            } => {
                base_solid_compression_threshold
                    * multiplier_per_level.powf(context.levels_below as f32)
            }
            Self::Custom(threshold) => threshold(context),
        }
    }
}

/// Inputs available to a custom [`CompressionThresholdSchedule`] function.
#[derive(Clone, Copy)]
pub struct CompressionThresholdContext {
    pub base_solid_compression_threshold: f32,
    pub depth_from_root: usize,
    pub levels_below: usize,
    pub density: f32,
    pub child_densities: [f32; 8],
}

/// Controls when an internal octree node can replace its descendants with a solid node.
#[derive(Clone, Copy)]
pub struct CompressionSettings {
    pub base_solid_compression_threshold: f32,
    pub density_aggregation: DensityAggregation,
    pub threshold_schedule: CompressionThresholdSchedule,
}

impl CompressionSettings {
    /// Disables solid compression while retaining density metadata in the cache.
    pub const fn disabled() -> Self {
        Self {
            base_solid_compression_threshold: f32::INFINITY,
            density_aggregation: DensityAggregation::Accumulated,
            threshold_schedule: CompressionThresholdSchedule::Constant,
        }
    }

    /// Compresses only fully solid regions using accumulated descendant density.
    pub const fn full_solid() -> Self {
        Self {
            base_solid_compression_threshold: 1.0,
            density_aggregation: DensityAggregation::Accumulated,
            threshold_schedule: CompressionThresholdSchedule::Exponential {
                multiplier_per_level: 8.0,
            },
        }
    }
}

impl Default for CompressionSettings {
    fn default() -> Self {
        Self::disabled()
    }
}

/// A compact SVO node with no parent pointer or per-child allocation.
#[repr(C)]
pub struct OptimizedNode<T> {
    pub id: u32,
    /// Index of the first packed child, or [`NO_CHILDREN`] for a leaf.
    pub first_child: u32,
    /// One occupancy bit for each octant; children are packed by octant order.
    pub child_mask: u8,
    /// Derived density used to decide whether this node can become a solid node.
    pub density: f32,
    /// A solid node represents its whole region and intentionally omits descendants.
    pub is_compressed_solid: bool,
    pub data: T,
}

/// A compact immutable cache generated by breadth-first traversal of an octree.
pub struct OptimizedOctree<T> {
    /// The base density required before an internal node is stored as solid.
    pub base_solid_compression_threshold: f32,
    /// The rule used to derive density for every internal node.
    pub density_aggregation: DensityAggregation,
    /// The rule used to adjust the base threshold for larger octree regions.
    pub threshold_schedule: CompressionThresholdSchedule,
    /// Contiguous nodes; children occupy `first_child + rank_in_child_mask`.
    pub nodes: ReadOnlyBuffer<OptimizedNode<T>>,
    /// Start index for every LOD level, followed by the exclusive final index.
    pub level_offsets: ReadOnlyBuffer<u32>,
    pub root_index: u32,
}

impl<T> OptimizedOctree<T> {
    /// Resolves an octant to its compact child-node index.
    pub fn child_index(&self, node_index: u32, octant: u8) -> Option<u32> {
        if octant >= 8 {
            return None;
        }

        let node = self.nodes.get(usize::try_from(node_index).ok()?)?;
        let octant_bit = 1_u8 << octant;
        if node.child_mask & octant_bit == 0 {
            return None;
        }

        let preceding_children = u32::from((node.child_mask & (octant_bit - 1)).count_ones());
        let child_index = node.first_child.checked_add(preceding_children)?;

        self.nodes
            .get(usize::try_from(child_index).ok()?)
            .map(|_| child_index)
    }

    /// Resolves an octant path from the root without allocating traversal state.
    pub fn node_at_path(&self, path: &[u8]) -> Option<&OptimizedNode<T>> {
        let mut node_index = self.root_index;

        for &octant in path {
            node_index = self.child_index(node_index, octant)?;
        }

        self.nodes.get(usize::try_from(node_index).ok()?)
    }
}

impl<T: Clone + OctreeDensity> OptimizedOctree<T> {
    /// Builds an uncompressed cache with density read from [`OctreeDensity`].
    pub fn from_unoptimized(source: &UnoptimizedOctree<T>) -> Result<Self, OptimizedOctreeError> {
        Self::from_unoptimized_with_settings(source, CompressionSettings::default())
    }

    /// Builds a cache with density read from [`OctreeDensity`] and selected compression settings.
    pub fn from_unoptimized_with_settings(
        source: &UnoptimizedOctree<T>,
        settings: CompressionSettings,
    ) -> Result<Self, OptimizedOctreeError> {
        Self::from_unoptimized_with_density(source, settings, |data| data.density())
    }
}

impl<T: Clone> OptimizedOctree<T> {
    /// Builds a cache with an application-provided leaf-density function.
    pub fn from_unoptimized_with_density(
        source: &UnoptimizedOctree<T>,
        settings: CompressionSettings,
        density_of: fn(&T) -> f32,
    ) -> Result<Self, OptimizedOctreeError> {
        let maximum_depth = maximum_depth(&source.root);
        let compression_plan =
            CompressionPlan::from_node(&source.root, 0, maximum_depth, settings, density_of);
        let mut pending_nodes = VecDeque::from([&compression_plan]);
        let mut nodes = Vec::new();
        let mut level_offsets = vec![0];

        while !pending_nodes.is_empty() {
            let nodes_in_level = pending_nodes.len();

            for _ in 0..nodes_in_level {
                let plan_node = pending_nodes
                    .pop_front()
                    .expect("the breadth-first traversal queue cannot be empty");
                let mut child_mask = 0_u8;

                for (octant, child) in plan_node.children.iter().enumerate() {
                    if child.is_some() {
                        child_mask |= 1_u8 << octant;
                    }
                }

                let first_child = if child_mask == 0 {
                    NO_CHILDREN
                } else {
                    let first_child_index = nodes
                        .len()
                        .checked_add(pending_nodes.len())
                        .and_then(|index| index.checked_add(1))
                        .ok_or(OptimizedOctreeError::NodeCountExceedsIndexRange)?;
                    compact_index(first_child_index)?
                };

                nodes.push(OptimizedNode {
                    id: plan_node.source.id,
                    first_child,
                    child_mask,
                    density: plan_node.density,
                    is_compressed_solid: plan_node.is_compressed_solid,
                    data: plan_node.source.data.clone(),
                });

                for child in plan_node.children.iter().flatten() {
                    pending_nodes.push_back(child.as_ref());
                }
            }

            level_offsets.push(compact_index(nodes.len())?);
        }

        Ok(Self {
            base_solid_compression_threshold: settings.base_solid_compression_threshold,
            density_aggregation: settings.density_aggregation,
            threshold_schedule: settings.threshold_schedule,
            nodes: ReadOnlyBuffer::from(nodes),
            level_offsets: ReadOnlyBuffer::from(level_offsets),
            root_index: 0,
        })
    }
}

impl<T: Clone + OctreeDensity> TryFrom<&UnoptimizedOctree<T>> for OptimizedOctree<T> {
    type Error = OptimizedOctreeError;

    fn try_from(source: &UnoptimizedOctree<T>) -> Result<Self, Self::Error> {
        Self::from_unoptimized(source)
    }
}

struct CompressionPlan<'tree, T> {
    source: &'tree Node<T, 8>,
    density: f32,
    is_compressed_solid: bool,
    children: [Option<Box<CompressionPlan<'tree, T>>>; 8],
}

impl<'tree, T> CompressionPlan<'tree, T> {
    fn from_node(
        source: &'tree Node<T, 8>,
        depth_from_root: usize,
        maximum_depth: usize,
        settings: CompressionSettings,
        density_of: fn(&T) -> f32,
    ) -> Self {
        let mut children = std::array::from_fn(|_| None);
        let mut child_densities = [0.0; 8];

        for (octant, child) in source.children.iter().enumerate() {
            let Some(child) = child.as_deref() else {
                continue;
            };
            let child_plan = Self::from_node(
                child,
                depth_from_root + 1,
                maximum_depth,
                settings,
                density_of,
            );

            child_densities[octant] = child_plan.density;
            children[octant] = Some(Box::new(child_plan));
        }

        if children.iter().all(Option::is_none) {
            return Self {
                source,
                density: sanitize_density(density_of(&source.data)),
                is_compressed_solid: false,
                children,
            };
        }

        let levels_below = maximum_depth.saturating_sub(depth_from_root);
        let density_context = DensityAggregationContext {
            depth_from_root,
            levels_below,
            child_densities,
        };
        let density = settings.density_aggregation.aggregate(density_context);
        let threshold_context = CompressionThresholdContext {
            base_solid_compression_threshold: settings.base_solid_compression_threshold,
            depth_from_root,
            levels_below,
            density,
            child_densities,
        };
        let threshold = settings
            .threshold_schedule
            .threshold(settings.base_solid_compression_threshold, threshold_context);
        let is_compressed_solid = density > 0.0 && density >= threshold;

        if is_compressed_solid {
            children = std::array::from_fn(|_| None);
        }

        Self {
            source,
            density,
            is_compressed_solid,
            children,
        }
    }
}

fn maximum_depth<T>(node: &Node<T, 8>) -> usize {
    node.children
        .iter()
        .flatten()
        .map(|child| maximum_depth(child) + 1)
        .max()
        .unwrap_or(0)
}

fn sanitize_density(density: f32) -> f32 {
    if density.is_finite() {
        density.max(0.0)
    } else {
        0.0
    }
}

fn compact_index(index: usize) -> Result<u32, OptimizedOctreeError> {
    u32::try_from(index).map_err(|_| OptimizedOctreeError::NodeCountExceedsIndexRange)
}

/// Immutable contiguous storage used by [`OptimizedOctree`].
pub struct ReadOnlyBuffer<T> {
    // Kept private so the optimized cache cannot mutate in place.
    values: Box<[T]>,
}

impl<T> From<Vec<T>> for ReadOnlyBuffer<T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values: values.into_boxed_slice(),
        }
    }
}

impl<T> Deref for ReadOnlyBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
