use super::{
    error::{OctreeError, OptimizedOctreeError},
    optimized::{CompressionSettings, OctreeDensity, OptimizedOctree},
    unoptimized::{Node, UnoptimizedOctree},
};

/// An editable octree and its optional immutable compact cache.
///
/// Source and cache representations are private; shared getters allow inspection
/// without structural mutation or value replacement. `&T` and `T::Clone` do not
/// freeze payloads with interior mutability or shared aliases. A stable cache
/// snapshot requires payload logical values not to change outside controlled edits.
pub struct Octree<T> {
    /// Source tree used by CRUD operations and cache generation.
    unoptimized_octree: UnoptimizedOctree<T>,
    /// Cache generated from `unoptimized_octree`; mutations invalidate it.
    optimized_octree: Option<OptimizedOctree<T>>,
}

impl<T> Octree<T> {
    /// Creates an octree containing only its root node.
    pub fn new(root_id: u32, root_data: T) -> Self {
        Self {
            unoptimized_octree: UnoptimizedOctree::new(root_id, root_data),
            optimized_octree: None,
        }
    }

    /// Borrows the editable source without granting structural mutation.
    pub fn source(&self) -> &UnoptimizedOctree<T> {
        &self.unoptimized_octree
    }

    /// Borrows the last explicitly built cache, without rebuilding it.
    pub fn optimized_cache(&self) -> Option<&OptimizedOctree<T>> {
        self.optimized_octree.as_ref()
    }

    fn invalidate_optimized_cache(&mut self) {
        self.optimized_octree = None;
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

        self.invalidate_optimized_cache();
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

        self.invalidate_optimized_cache();
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

        self.invalidate_optimized_cache();
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

/// A sparse, fixed-depth leaf-value owner with reusable heap-path node IDs.
///
/// The root always has ID 0. Missing paths represent `empty_value`; intermediate
/// nodes carry that empty value. Only complete paths can be edited, and clearing
/// a leaf prunes empty ancestors without removing the root. Depths 0..=10 admit
/// every path in the `u32` ID range. There is no cumulative allocation budget.
///
/// A changed value invalidates the shared compact cache; an equal value, missing
/// empty write, or invalid path preserves both representations. Rebuilding is
/// explicit and publishes only after successful conversion. Source navigation,
/// chain creation and pruning are O(depth); payload Clone/PartialEq/Drop and old
/// cache destruction costs are additional. `len` separately walks the source.
///
/// Shared source access protects structure and value replacement, not logical
/// payload immutability. `&T` and `T::Clone` do not freeze Cell/Mutex values or
/// shared aliases. For a stable cache snapshot, payloads must not change logical
/// values outside controlled edits.
/// Arbitrary-ID insertion is not an operation of this owner.
pub struct FixedDepthOctree<T> {
    tree: Octree<T>,
    depth: u8,
    empty_value: T,
}

impl<T> FixedDepthOctree<T> {
    /// Counts the root and descendants in O(n), independently of the cache.
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    /// Borrows the source without exposing a structural edit capability.
    pub fn source(&self) -> &UnoptimizedOctree<T> {
        self.tree.source()
    }

    /// Borrows the last explicitly rebuilt cache, if still current.
    pub fn optimized_cache(&self) -> Option<&OptimizedOctree<T>> {
        self.tree.optimized_cache()
    }
}

impl<T: Clone> FixedDepthOctree<T> {
    /// Creates an empty root after verifying all path IDs fit in `u32`.
    pub fn new(depth: u8, empty_value: T) -> Result<Self, OctreeError> {
        let mut maximum_id = 0_u32;
        for _ in 0..depth {
            maximum_id = maximum_id
                .checked_mul(8)
                .and_then(|id| id.checked_add(8))
                .ok_or(OctreeError::FixedDepthExceedsIdRange { depth })?;
        }
        Ok(Self {
            tree: Octree::new(0, empty_value.clone()),
            depth,
            empty_value,
        })
    }

    /// Explicitly rebuilds with the existing density conversion and publication policy.
    pub fn rebuild_optimized_cache_with_density(
        &mut self,
        settings: CompressionSettings,
        density_of: fn(&T) -> f32,
    ) -> Result<(), OptimizedOctreeError> {
        self.tree
            .rebuild_optimized_cache_with_density(settings, density_of)
    }
}

impl<T: Clone + PartialEq> FixedDepthOctree<T> {
    /// Edits a complete path, returning the previous value only on change.
    ///
    /// Length is validated before every octant, from left to right. Invalid
    /// paths and no-ops leave source and cache intact. Missing nonempty paths
    /// are prepared off-tree before invalidation and attachment.
    pub fn set(&mut self, path: &[u8], value: T) -> Result<Option<T>, OctreeError> {
        if path.len() != usize::from(self.depth) {
            return Err(OctreeError::PathDepthMismatch {
                expected: self.depth,
                actual: path.len(),
            });
        }
        for &octant in path {
            if octant >= 8 {
                return Err(OctreeError::ChildIndexOutOfBounds {
                    index: u32::from(octant),
                });
            }
        }

        let mut node = &self.tree.unoptimized_octree.root;
        for (offset, &octant) in path.iter().enumerate() {
            let Some(child) = node.children[usize::from(octant)].as_deref() else {
                if value == self.empty_value {
                    return Ok(None);
                }
                let previous = self.empty_value.clone();
                let chain = Self::prepare_chain(node.id, &path[offset..], value, &self.empty_value);
                self.tree.invalidate_optimized_cache();
                let mut parent = &mut self.tree.unoptimized_octree.root;
                for &prefix_octant in &path[..offset] {
                    parent = parent.children[usize::from(prefix_octant)]
                        .as_deref_mut()
                        .expect("the pre-read fixed path prefix exists");
                }
                parent.children[usize::from(octant)] = Some(chain);
                return Ok(Some(previous));
            };
            node = child;
        }

        if node.data == value {
            return Ok(None);
        }
        let clearing = value == self.empty_value;
        self.tree.invalidate_optimized_cache();
        let previous = Self::replace_path(
            &mut self.tree.unoptimized_octree.root,
            path,
            value,
            &self.empty_value,
            clearing,
        );
        Ok(Some(previous))
    }

    fn prepare_chain(parent_id: u32, path: &[u8], value: T, empty_value: &T) -> Box<Node<T, 8>> {
        let octant = path[0];
        let id = parent_id
            .checked_mul(8)
            .and_then(|id| id.checked_add(u32::from(octant) + 1))
            .expect("fixed depth admission proves every heap-path ID fits in u32");
        if path.len() == 1 {
            return Box::new(Node::new(id, value));
        }
        let mut node = Node::new(id, empty_value.clone());
        node.children[usize::from(path[1])] =
            Some(Self::prepare_chain(id, &path[1..], value, empty_value));
        Box::new(node)
    }

    fn replace_path(
        node: &mut Node<T, 8>,
        path: &[u8],
        value: T,
        empty_value: &T,
        clearing: bool,
    ) -> T {
        let Some((&octant, remaining)) = path.split_first() else {
            return std::mem::replace(&mut node.data, value);
        };
        let slot = &mut node.children[usize::from(octant)];
        let child = slot.as_deref_mut().expect("the pre-read fixed path exists");
        let previous = Self::replace_path(child, remaining, value, empty_value, clearing);
        if clearing && child.data == *empty_value && child.children.iter().all(Option::is_none) {
            *slot = None;
        }
        previous
    }
}
