//! Deterministic ordered rooted-tree storage and structural validation.
//!
//! This module owns graph topology only. Domain modules decide what nodes mean
//! and which edits should occur.

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootedTreeError {
    UnknownParent,
    UnknownNode,
    DuplicateNode,
    SelfParent,
    InvalidReparent,
    InvalidStructure(RootedTreeInvariantError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootedTreeInvariantError {
    MissingRoot,
    RootHasParent,
    MissingParent,
    MissingChild,
    ParentChildMismatch,
    DuplicateChild,
    RootHasParentReference,
    Cycle,
    DisconnectedNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeReparent<N> {
    pub node: N,
    pub from: N,
    pub to: N,
}

#[derive(Debug)]
struct Node<N, M> {
    metadata: M,
    parent: Option<N>,
    children: Vec<N>,
}

#[derive(Debug, Clone)]
struct TopologyNode<N> {
    parent: Option<N>,
    children: Vec<N>,
}

/// A single-root tree preserving child insertion and reparent order.
#[derive(Debug)]
pub struct OrderedRootedTree<N, M> {
    root: Option<N>,
    nodes: HashMap<N, Node<N, M>>,
}

impl<N, M> OrderedRootedTree<N, M>
where
    N: Copy + Eq + Hash,
{
    pub fn new(root: N, metadata: M) -> Self {
        Self {
            root: Some(root),
            nodes: HashMap::from([(
                root,
                Node {
                    metadata,
                    parent: None,
                    children: Vec::new(),
                },
            )]),
        }
    }

    pub fn root(&self) -> Option<N> {
        self.root
    }

    pub fn contains(&self, node: N) -> bool {
        self.nodes.contains_key(&node)
    }

    pub fn parent(&self, node: N) -> Option<N> {
        self.nodes.get(&node).and_then(|entry| entry.parent)
    }

    pub fn metadata(&self, node: N) -> Option<&M> {
        self.nodes.get(&node).map(|entry| &entry.metadata)
    }

    pub fn children(&self, node: N) -> impl Iterator<Item = N> + '_ {
        self.nodes
            .get(&node)
            .into_iter()
            .flat_map(|entry| entry.children.iter().copied())
    }

    pub fn insert_child(&mut self, parent: N, node: N, metadata: M) -> Result<(), RootedTreeError> {
        if parent == node {
            return Err(RootedTreeError::SelfParent);
        }
        if self.nodes.contains_key(&node) {
            return Err(RootedTreeError::DuplicateNode);
        }
        if !self.nodes.contains_key(&parent) {
            return Err(RootedTreeError::UnknownParent);
        }
        self.nodes.insert(
            node,
            Node {
                metadata,
                parent: Some(parent),
                children: Vec::new(),
            },
        );
        self.nodes
            .get_mut(&parent)
            .expect("parent was checked")
            .children
            .push(node);
        Ok(())
    }

    /// Atomically applies topology edits after validating their simulated result.
    pub fn apply_edits(
        &mut self,
        reparented: &[TreeReparent<N>],
        removed: &[N],
    ) -> Result<(), RootedTreeError> {
        self.validate().map_err(RootedTreeError::InvalidStructure)?;
        let mut topology = self.topology();
        let mut root = self.root;
        apply_topology_edits(&mut root, &mut topology, reparented, removed)?;
        validate_topology(root, &topology).map_err(RootedTreeError::InvalidStructure)?;

        for change in reparented {
            self.nodes
                .get_mut(&change.from)
                .expect("simulated edit validated source")
                .children
                .retain(|child| *child != change.node);
            self.nodes
                .get_mut(&change.node)
                .expect("simulated edit validated node")
                .parent = Some(change.to);
            self.nodes
                .get_mut(&change.to)
                .expect("simulated edit validated destination")
                .children
                .push(change.node);
        }
        let removed_set = removed.iter().copied().collect::<HashSet<_>>();
        for node in removed {
            if let Some(parent) = self.parent(*node) {
                if !removed_set.contains(&parent) {
                    self.nodes
                        .get_mut(&parent)
                        .expect("simulated edit validated parent")
                        .children
                        .retain(|child| child != node);
                }
            }
        }
        for node in removed {
            self.nodes.remove(node);
        }
        self.root = root;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), RootedTreeInvariantError> {
        validate_topology(self.root, &self.topology())
    }

    fn topology(&self) -> HashMap<N, TopologyNode<N>> {
        self.nodes
            .iter()
            .map(|(&id, node)| {
                (
                    id,
                    TopologyNode {
                        parent: node.parent,
                        children: node.children.clone(),
                    },
                )
            })
            .collect()
    }
}

fn apply_topology_edits<N>(
    root: &mut Option<N>,
    nodes: &mut HashMap<N, TopologyNode<N>>,
    reparented: &[TreeReparent<N>],
    removed: &[N],
) -> Result<(), RootedTreeError>
where
    N: Copy + Eq + Hash,
{
    let removed_set = removed.iter().copied().collect::<HashSet<_>>();
    if removed_set.len() != removed.len()
        || removed_set.iter().any(|node| !nodes.contains_key(node))
    {
        return Err(RootedTreeError::UnknownNode);
    }
    let mut moved = HashSet::new();
    for change in reparented {
        if !moved.insert(change.node)
            || removed_set.contains(&change.node)
            || removed_set.contains(&change.to)
            || nodes.get(&change.node).and_then(|node| node.parent) != Some(change.from)
            || !nodes.contains_key(&change.to)
        {
            return Err(RootedTreeError::InvalidReparent);
        }
        nodes
            .get_mut(&change.from)
            .expect("source checked")
            .children
            .retain(|child| *child != change.node);
        nodes.get_mut(&change.node).expect("node checked").parent = Some(change.to);
        nodes
            .get_mut(&change.to)
            .expect("destination checked")
            .children
            .push(change.node);
    }
    for node in removed {
        if let Some(parent) = nodes.get(node).and_then(|entry| entry.parent) {
            if !removed_set.contains(&parent) {
                nodes
                    .get_mut(&parent)
                    .expect("parent exists before removal")
                    .children
                    .retain(|child| child != node);
            }
        }
    }
    for node in removed {
        nodes.remove(node);
    }
    if root.is_some_and(|node| removed_set.contains(&node)) {
        *root = None;
    }
    Ok(())
}

fn validate_topology<N>(
    root: Option<N>,
    nodes: &HashMap<N, TopologyNode<N>>,
) -> Result<(), RootedTreeInvariantError>
where
    N: Copy + Eq + Hash,
{
    let Some(root) = root else {
        return if nodes.is_empty() {
            Ok(())
        } else {
            Err(RootedTreeInvariantError::MissingRoot)
        };
    };
    let root_entry = nodes
        .get(&root)
        .ok_or(RootedTreeInvariantError::MissingRoot)?;
    if root_entry.parent.is_some() {
        return Err(RootedTreeInvariantError::RootHasParent);
    }
    for (&node, entry) in nodes {
        if node != root && entry.parent.is_none() {
            return Err(RootedTreeInvariantError::MissingParent);
        }
        if let Some(parent) = entry.parent {
            let parent_entry = nodes
                .get(&parent)
                .ok_or(RootedTreeInvariantError::MissingParent)?;
            if !parent_entry.children.contains(&node) {
                return Err(RootedTreeInvariantError::ParentChildMismatch);
            }
        }
        let mut unique = HashSet::new();
        for &child in &entry.children {
            if !unique.insert(child) {
                return Err(RootedTreeInvariantError::DuplicateChild);
            }
            if child == root {
                return Err(RootedTreeInvariantError::RootHasParentReference);
            }
            let child_entry = nodes
                .get(&child)
                .ok_or(RootedTreeInvariantError::MissingChild)?;
            if child_entry.parent != Some(node) {
                return Err(RootedTreeInvariantError::ParentChildMismatch);
            }
        }
    }
    let mut visited = HashSet::new();
    validate_reachable(root, nodes, &mut visited, &mut HashSet::new())?;
    if visited.len() != nodes.len() {
        return Err(RootedTreeInvariantError::DisconnectedNode);
    }
    Ok(())
}

fn validate_reachable<N>(
    node: N,
    nodes: &HashMap<N, TopologyNode<N>>,
    visited: &mut HashSet<N>,
    active: &mut HashSet<N>,
) -> Result<(), RootedTreeInvariantError>
where
    N: Copy + Eq + Hash,
{
    if !active.insert(node) || !visited.insert(node) {
        return Err(RootedTreeInvariantError::Cycle);
    }
    for &child in &nodes[&node].children {
        validate_reachable(child, nodes, visited, active)?;
    }
    active.remove(&node);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ordered_insertion_and_atomic_edits() {
        let mut tree = OrderedRootedTree::new(0, ());
        tree.insert_child(0, 1, ()).unwrap();
        tree.insert_child(1, 2, ()).unwrap();
        tree.insert_child(1, 3, ()).unwrap();
        tree.apply_edits(
            &[TreeReparent {
                node: 3,
                from: 1,
                to: 0,
            }],
            &[1, 2],
        )
        .unwrap();
        assert_eq!(tree.children(0).collect::<Vec<_>>(), vec![3]);
        assert_eq!(tree.parent(3), Some(0));
        tree.validate().unwrap();
    }

    #[test]
    fn rejected_edits_do_not_mutate_the_tree() {
        let mut tree = OrderedRootedTree::new(0, ());
        tree.insert_child(0, 1, ()).unwrap();
        let before = tree.children(0).collect::<Vec<_>>();
        assert_eq!(
            tree.apply_edits(
                &[TreeReparent {
                    node: 1,
                    from: 9,
                    to: 0,
                }],
                &[],
            ),
            Err(RootedTreeError::InvalidReparent)
        );
        assert_eq!(tree.children(0).collect::<Vec<_>>(), before);
    }
}
