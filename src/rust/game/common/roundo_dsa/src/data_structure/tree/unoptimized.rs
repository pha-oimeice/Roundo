use super::error::OctreeError;

/// A node in an N-ary tree. `Octree` uses `N = 8`.
pub struct Node<T, const N: usize> {
    pub id: u32,
    pub data: T,
    pub children: [Option<Box<Node<T, N>>>; N],
}

impl<T, const N: usize> Node<T, N> {
    pub fn new(id: u32, data: T) -> Self {
        Self {
            id,
            data,
            children: std::array::from_fn(|_| None),
        }
    }

    /// Visits the node before recursively visiting its children.
    pub fn prefix_traversal(&self) -> Vec<&Self> {
        let mut nodes = Vec::new();
        self.collect_prefix(&mut nodes);
        nodes
    }

    /// Visits the first half of child slots, the node, then the remaining child slots.
    pub fn infix_traversal(&self) -> Vec<&Self> {
        let mut nodes = Vec::new();
        self.collect_infix(&mut nodes);
        nodes
    }

    /// Visits all children before the node itself.
    pub fn postfix_traversal(&self) -> Vec<&Self> {
        let mut nodes = Vec::new();
        self.collect_postfix(&mut nodes);
        nodes
    }

    pub(crate) fn child_slot(index: u32) -> Result<usize, OctreeError> {
        let slot = index as usize;
        if slot < N {
            Ok(slot)
        } else {
            Err(OctreeError::ChildIndexOutOfBounds { index })
        }
    }

    pub(crate) fn subtree_len(&self) -> usize {
        1 + self
            .children
            .iter()
            .flatten()
            .map(|child| child.subtree_len())
            .sum::<usize>()
    }

    pub(crate) fn find_by_id(&self, id: u32) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }

        self.children
            .iter()
            .flatten()
            .find_map(|child| child.find_by_id(id))
    }

    pub(crate) fn find_by_id_mut(&mut self, id: u32) -> Option<&mut Self> {
        if self.id == id {
            return Some(self);
        }

        for child in &mut self.children {
            if let Some(child) = child.as_deref_mut()
                && let Some(node) = child.find_by_id_mut(id)
            {
                return Some(node);
            }
        }

        None
    }

    pub(crate) fn remove_descendant(&mut self, id: u32) -> Option<Box<Self>> {
        for child_slot in &mut self.children {
            if child_slot.as_ref().is_some_and(|child| child.id == id) {
                return child_slot.take();
            }

            if let Some(child) = child_slot.as_deref_mut()
                && let Some(node) = child.remove_descendant(id)
            {
                return Some(node);
            }
        }

        None
    }

    fn collect_prefix<'node>(&'node self, nodes: &mut Vec<&'node Self>) {
        nodes.push(self);
        for child in self.children.iter().flatten() {
            child.collect_prefix(nodes);
        }
    }

    fn collect_infix<'node>(&'node self, nodes: &mut Vec<&'node Self>) {
        let middle = N / 2;

        for child in self.children[..middle].iter().flatten() {
            child.collect_infix(nodes);
        }

        nodes.push(self);

        for child in self.children[middle..].iter().flatten() {
            child.collect_infix(nodes);
        }
    }

    fn collect_postfix<'node>(&'node self, nodes: &mut Vec<&'node Self>) {
        for child in self.children.iter().flatten() {
            child.collect_postfix(nodes);
        }

        nodes.push(self);
    }
}

/// The editable, pointer-based source representation for an octree.
pub struct UnoptimizedOctree<T> {
    pub root: Node<T, 8>,
}

impl<T> UnoptimizedOctree<T> {
    pub fn new(root_id: u32, root_data: T) -> Self {
        Self {
            root: Node::new(root_id, root_data),
        }
    }
}
