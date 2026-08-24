use super::{
    error::{OctreeError, OptimizedOctreeError},
    optimized::{CompressionSettings, OctreeDensity, OptimizedOctree},
    unoptimized::{Node, UnoptimizedOctree},
};

/// An editable octree and its optional immutable compact cache.
pub struct Octree<T> {
    /// Source tree used by CRUD operations and cache generation.
    pub unoptimized_octree: UnoptimizedOctree<T>,
    /// Cache generated from `unoptimized_octree`; mutations invalidate it.
    pub optimized_octree: Option<OptimizedOctree<T>>,
}

impl<T> Octree<T> {
    /// Creates an octree containing only its root node.
    pub fn new(root_id: u32, root_data: T) -> Self {
        Self {
            unoptimized_octree: UnoptimizedOctree::new(root_id, root_data),
            optimized_octree: None,
        }
    }

    pub fn len(&self) -> usize {
        self.unoptimized_octree.root.subtree_len()
    }

    pub fn contains(&self, id: u32) -> bool {
        self.get_node(id).is_some()
    }

    /// Inserts a node into an empty child slot of an existing parent.
    pub fn insert(
        &mut self,
        parent_id: u32,
        child_index: u8,
        id: u32,
        data: T,
    ) -> Result<(), OctreeError> {
        let child_index = u32::from(child_index);
        let child_slot = Node::<T, 8>::child_slot(child_index)?;

        if self.contains(id) {
            return Err(OctreeError::DuplicateNodeId { id });
        }

        {
            let parent = self
                .unoptimized_octree
                .root
                .find_by_id_mut(parent_id)
                .ok_or(OctreeError::NodeNotFound { id: parent_id })?;

            if parent.children[child_slot].is_some() {
                return Err(OctreeError::ChildSlotOccupied {
                    parent_id,
                    child_index,
                });
            }

            parent.children[child_slot] = Some(Box::new(Node::new(id, data)));
        }

        self.optimized_octree = None;
        Ok(())
    }

    pub fn get(&self, id: u32) -> Option<&T> {
        self.get_node(id).map(|node| &node.data)
    }

    pub fn get_node(&self, id: u32) -> Option<&Node<T, 8>> {
        self.unoptimized_octree.root.find_by_id(id)
    }

    /// Replaces a node's value and returns the previous value.
    pub fn update(&mut self, id: u32, data: T) -> Result<T, OctreeError> {
        let previous_data = {
            let node = self
                .unoptimized_octree
                .root
                .find_by_id_mut(id)
                .ok_or(OctreeError::NodeNotFound { id })?;

            std::mem::replace(&mut node.data, data)
        };

        self.optimized_octree = None;
        Ok(previous_data)
    }

    /// Removes a node and all of its descendants, returning the removed node's value.
    pub fn remove(&mut self, id: u32) -> Result<T, OctreeError> {
        self.remove_subtree(id).map(|node| node.data)
    }

    /// Removes a node and all of its descendants, returning the detached subtree.
    pub fn remove_subtree(&mut self, id: u32) -> Result<Node<T, 8>, OctreeError> {
        if self.unoptimized_octree.root.id == id {
            return Err(OctreeError::CannotRemoveRoot);
        }

        let removed_node = self
            .unoptimized_octree
            .root
            .remove_descendant(id)
            .map(|node| *node)
            .ok_or(OctreeError::NodeNotFound { id })?;

        self.optimized_octree = None;
        Ok(removed_node)
    }

    /// Visits each node before visiting its children.
    pub fn prefix_traversal(&self) -> Vec<&Node<T, 8>> {
        self.unoptimized_octree.root.prefix_traversal()
    }

    /// Visits the first four child slots, then the node, then the final four child slots.
    pub fn infix_traversal(&self) -> Vec<&Node<T, 8>> {
        self.unoptimized_octree.root.infix_traversal()
    }

    /// Visits each node after visiting all of its children.
    pub fn postfix_traversal(&self) -> Vec<&Node<T, 8>> {
        self.unoptimized_octree.root.postfix_traversal()
    }
}

impl<T: Clone + OctreeDensity> Octree<T> {
    /// Rebuilds the immutable cache without solid-node compression.
    pub fn rebuild_optimized_cache(&mut self) -> Result<(), OptimizedOctreeError> {
        self.optimized_octree = Some(OptimizedOctree::from_unoptimized(&self.unoptimized_octree)?);
        Ok(())
    }

    /// Rebuilds the cache with the selected solid-compression settings.
    pub fn rebuild_optimized_cache_with_settings(
        &mut self,
        settings: CompressionSettings,
    ) -> Result<(), OptimizedOctreeError> {
        self.optimized_octree = Some(OptimizedOctree::from_unoptimized_with_settings(
            &self.unoptimized_octree,
            settings,
        )?);
        Ok(())
    }
}

impl<T: Clone> Octree<T> {
    /// Rebuilds the cache using an application-provided leaf-density function.
    pub fn rebuild_optimized_cache_with_density(
        &mut self,
        settings: CompressionSettings,
        density_of: fn(&T) -> f32,
    ) -> Result<(), OptimizedOctreeError> {
        self.optimized_octree = Some(OptimizedOctree::from_unoptimized_with_density(
            &self.unoptimized_octree,
            settings,
            density_of,
        )?);
        Ok(())
    }
}
