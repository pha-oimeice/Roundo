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

    /// Counts the root and every descendant in the editable source tree.
    ///
    /// This walks the tree and is `O(n)`; the compact cache is not consulted.
    pub fn len(&self) -> usize {
        self.unoptimized_octree.root.subtree_len()
    }

    /// Returns whether `id` occurs anywhere in the editable source tree.
    ///
    /// IDs are unique while the tree is valid. This performs a tree search and
    /// does not use the compact cache.
    pub fn contains(&self, id: u32) -> bool {
        self.get_node(id).is_some()
    }

    /// Inserts a node into an empty child slot of an existing parent.
    ///
    /// On success, the new ID is unique and any compact cache is invalidated.
    /// An invalid child index, missing parent, duplicate ID, or occupied slot
    /// returns an error without changing either representation.
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

    /// Borrows a node value from the editable source tree.
    ///
    /// Returns `None` when `id` is absent. The borrow remains valid until the
    /// tree is mutably borrowed.
    pub fn data(&self, id: u32) -> Option<&T> {
        self.get_node(id).map(|node| &node.data)
    }

    /// Borrows a complete source node, including its subtree.
    ///
    /// This exposes editable-tree topology, not compact-cache indices. Returns
    /// `None` when `id` is absent.
    pub fn get_node(&self, id: u32) -> Option<&Node<T, 8>> {
        self.unoptimized_octree.root.find_by_id(id)
    }

    /// Replaces a node's value and returns the previous value.
    ///
    /// A successful replacement invalidates the compact cache. A missing ID
    /// returns an error without changing the tree or cache.
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
    ///
    /// Descendant values are dropped. The root cannot be removed. Success
    /// invalidates the compact cache; failure leaves both representations intact.
    pub fn remove_value(&mut self, id: u32) -> Result<T, OctreeError> {
        self.remove_subtree(id).map(|node| node.data)
    }

    /// Removes a node and all of its descendants, returning the detached subtree.
    ///
    /// The returned node owns its descendants and no longer belongs to this
    /// tree. The root cannot be removed. Success invalidates the compact cache;
    /// failure leaves both representations intact.
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

    /// Returns borrowed nodes in prefix order, visiting child slots from 0 to 7.
    ///
    /// The returned vector allocates `O(n)` references and is invalidated by a
    /// later mutable borrow of the tree.
    pub fn prefix_traversal(&self) -> Vec<&Node<T, 8>> {
        self.unoptimized_octree.root.prefix_traversal()
    }

    /// Returns borrowed nodes after child slots 0..4 and before slots 4..8.
    ///
    /// The returned vector allocates `O(n)` references and is invalidated by a
    /// later mutable borrow of the tree.
    pub fn infix_traversal(&self) -> Vec<&Node<T, 8>> {
        self.unoptimized_octree.root.infix_traversal()
    }

    /// Returns borrowed nodes in postfix order, visiting child slots from 0 to 7.
    ///
    /// The returned vector allocates `O(n)` references and is invalidated by a
    /// later mutable borrow of the tree.
    pub fn postfix_traversal(&self) -> Vec<&Node<T, 8>> {
        self.unoptimized_octree.root.postfix_traversal()
    }
}

impl<T: Clone + OctreeDensity> Octree<T> {
    /// Rebuilds the immutable cache without solid-node compression.
    ///
    /// The cache is published only after conversion succeeds, so an error
    /// preserves the previous cache.
    pub fn rebuild_optimized_cache(&mut self) -> Result<(), OptimizedOctreeError> {
        self.optimized_octree = Some(OptimizedOctree::from_unoptimized(&self.unoptimized_octree)?);
        Ok(())
    }

    /// Rebuilds the cache with the selected solid-compression settings.
    ///
    /// The cache is published only after conversion succeeds. Compact indices
    /// and borrowed cache nodes must not be retained across a successful rebuild.
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
    ///
    /// The function is called synchronously while traversing the source tree.
    /// The cache is published only after conversion succeeds.
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
