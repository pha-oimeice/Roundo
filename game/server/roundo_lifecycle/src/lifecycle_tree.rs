//! Pure ownership/lifetime tree used by UI lifecycle adapters.
//!
//! The tree deliberately has no ECS, scheduling, or resource-cleanup dependency.
//! A root-unrestricted commit leaves a terminal empty tree; a root replacement owner
//! creates its replacement tree separately.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TREE_ID: AtomicU64 = AtomicU64::new(1);

/// Rejected tree query, mutation, or destruction transaction.
///
/// Planning errors never mutate the tree. [`AnchorTree::apply_plan`] validates
/// plan identity and invariants before structural mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    UnknownParent,
    UnknownTarget,
    DuplicateNode,
    SelfParent,
    RootDestructionRestricted,
    PlanDoesNotMatchTree,
    InvalidTree(TreeInvariantError),
}

/// Exact invariant violated by [`AnchorTree::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeInvariantError {
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

/// One surviving subtree root moved from `from` to `to` during destruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reparented<N> {
    pub node: N,
    pub from: N,
    pub to: N,
}

/// Deterministically ordered structural effects of a committed destruction.
///
/// `destroyed` follows tree traversal order; `reparented` contains only
/// survival-boundary roots, whose descendants move with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestructionOutcome<N> {
    pub destroyed: Vec<N>,
    pub reparented: Vec<Reparented<N>>,
}

/// An immutable, validated destruction transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestructionPlan<N> {
    tree_id: u64,
    version: u64,
    outcome: DestructionOutcome<N>,
}

impl<N> DestructionPlan<N> {
    /// Nodes that would be removed if this still-current plan is committed.
    pub fn destroyed(&self) -> &[N] {
        &self.outcome.destroyed
    }

    /// Surviving subtree roots that would be reparented on commit.
    pub fn reparented(&self) -> &[Reparented<N>] {
        &self.outcome.reparented
    }
}

#[derive(Debug)]
struct Node<N, M> {
    metadata: M,
    parent: Option<N>,
    children: Vec<N>,
}

/// A single-root ownership tree with deterministic insertion and traversal order.
#[derive(Debug)]
pub struct AnchorTree<N, M> {
    root: Option<N>,
    nodes: HashMap<N, Node<N, M>>,
    tree_id: u64,
    version: u64,
}

impl<N, M> AnchorTree<N, M>
where
    N: Copy + Eq + Hash,
{
    /// Creates a valid one-node tree with a fresh process-local tree identity.
    pub fn new(root: N, metadata: M) -> Self {
        let tree_id = NEXT_TREE_ID.fetch_add(1, Ordering::Relaxed);
        let mut nodes = HashMap::new();
        nodes.insert(
            root,
            Node {
                metadata,
                parent: None,
                children: Vec::new(),
            },
        );
        Self {
            root: Some(root),
            nodes,
            tree_id,
            version: 0,
        }
    }

    /// The root is absent only after an unrestricted root destruction commit.
    pub fn root(&self) -> Option<N> {
        self.root
    }

    /// Returns whether `node` is live in this tree.
    pub fn contains(&self, node: N) -> bool {
        self.nodes.contains_key(&node)
    }

    /// Returns the parent of a live non-root node.
    ///
    /// `None` means root or unknown; use [`contains`](Self::contains) to
    /// distinguish those cases.
    pub fn parent(&self, node: N) -> Option<N> {
        let entry = self.nodes.get(&node);
        entry.and_then(|entry| entry.parent)
    }

    /// Borrows metadata for a live node, or returns `None` when unknown.
    pub fn metadata(&self, node: N) -> Option<&M> {
        let entry = self.nodes.get(&node);
        entry.map(|entry| &entry.metadata)
    }

    /// Iterates direct children in insertion/reparent order.
    ///
    /// An unknown node produces an empty iterator. The iterator borrows the tree
    /// and cannot outlive a mutable tree operation.
    pub fn children(&self, node: N) -> impl Iterator<Item = N> + '_ {
        let entry = self.nodes.get(&node);
        entry
            .into_iter()
            .flat_map(|entry| entry.children.iter().copied())
    }

    /// Appends a new leaf to `parent`'s ordered children.
    ///
    /// Self-parenting, duplicate identity, and an unknown parent are rejected
    /// before mutation. Success advances the tree version and invalidates every
    /// outstanding [`DestructionPlan`] for this tree.
    pub fn insert_child(&mut self, parent: N, node: N, metadata: M) -> Result<(), TreeError> {
        if parent == node {
            return Err(TreeError::SelfParent);
        }
        if self.nodes.contains_key(&node) {
            return Err(TreeError::DuplicateNode);
        }
        if !self.nodes.contains_key(&parent) {
            return Err(TreeError::UnknownParent);
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
        self.version += 1;
        Ok(())
    }

    /// Calculates a complete transaction without changing this tree.
    ///
    /// Duplicate explicit targets are deduplicated. An explicitly targeted
    /// independent node is still destroyed; independence only protects the
    /// first non-target boundary below a destruction cascade. If root is
    /// targeted, `root_is_unrestricted` must be true and no descendant survives.
    pub fn plan_destruction<I, F>(
        &self,
        explicit_targets: I,
        root_is_unrestricted: bool,
        is_independent: F,
    ) -> Result<DestructionPlan<N>, TreeError>
    where
        I: IntoIterator<Item = N>,
        F: Fn(N, &M) -> bool,
    {
        self.validate().map_err(TreeError::InvalidTree)?;
        let targets: HashSet<N> = explicit_targets.into_iter().collect();
        for target in &targets {
            if !self.nodes.contains_key(target) {
                return Err(TreeError::UnknownTarget);
            }
        }
        let Some(root) = self.root else {
            return Ok(DestructionPlan {
                tree_id: self.tree_id,
                version: self.version,
                outcome: DestructionOutcome {
                    destroyed: Vec::new(),
                    reparented: Vec::new(),
                },
            });
        };
        if targets.contains(&root) && !root_is_unrestricted {
            return Err(TreeError::RootDestructionRestricted);
        }
        let mut outcome = DestructionOutcome {
            destroyed: Vec::new(),
            reparented: Vec::new(),
        };
        if targets.contains(&root) {
            self.collect_all(root, &mut outcome.destroyed);
        } else {
            self.plan_visit(root, None, &targets, &is_independent, &mut outcome);
        }
        Ok(DestructionPlan {
            tree_id: self.tree_id,
            version: self.version,
            outcome,
        })
    }

    /// Atomically applies a plan created from this exact, unchanged tree.
    ///
    /// A plan from another tree or an older version returns
    /// [`TreeError::PlanDoesNotMatchTree`] before mutation. Success advances the
    /// version, invalidating all other plans, and may leave the terminal empty
    /// tree when the root was unrestricted.
    pub fn apply_plan(
        &mut self,
        plan: DestructionPlan<N>,
    ) -> Result<DestructionOutcome<N>, TreeError> {
        if plan.tree_id != self.tree_id || plan.version != self.version {
            return Err(TreeError::PlanDoesNotMatchTree);
        }
        self.validate().map_err(TreeError::InvalidTree)?;
        let outcome = plan.outcome;
        let destroyed: HashSet<N> = outcome.destroyed.iter().copied().collect();

        for change in &outcome.reparented {
            if destroyed.contains(&change.node)
                || destroyed.contains(&change.to)
                || self.parent(change.node) != Some(change.from)
            {
                return Err(TreeError::PlanDoesNotMatchTree);
            }
        }
        for change in &outcome.reparented {
            self.nodes
                .get_mut(&change.from)
                .expect("validated plan")
                .children
                .retain(|&child| child != change.node);
            self.nodes
                .get_mut(&change.node)
                .expect("validated plan")
                .parent = Some(change.to);
            self.nodes
                .get_mut(&change.to)
                .expect("validated plan")
                .children
                .push(change.node);
        }
        // Explicit targets may occur below a surviving boundary. Remove those
        // target links from their surviving parents before dropping their nodes.
        for node in &outcome.destroyed {
            if let Some(parent) = self.parent(*node) {
                if !destroyed.contains(&parent) {
                    self.nodes
                        .get_mut(&parent)
                        .expect("validated plan")
                        .children
                        .retain(|&child| child != *node);
                }
            }
        }
        for node in &outcome.destroyed {
            let removed = self.nodes.remove(node);
            debug_assert!(removed.is_some(), "validated destruction target must exist");
        }
        if self.root.is_some_and(|root| destroyed.contains(&root)) {
            self.root = None;
        }
        self.version += 1;
        self.validate().map_err(TreeError::InvalidTree)?;
        Ok(outcome)
    }

    /// Verifies all internal edges rather than attempting to repair invalid state.
    pub fn validate(&self) -> Result<(), TreeInvariantError> {
        let Some(root) = self.root else {
            return if self.nodes.is_empty() {
                Ok(())
            } else {
                Err(TreeInvariantError::MissingRoot)
            };
        };
        let root_entry = self.nodes.get(&root);
        let root_entry = root_entry.ok_or(TreeInvariantError::MissingRoot)?;
        if root_entry.parent.is_some() {
            return Err(TreeInvariantError::RootHasParent);
        }
        for (&node, entry) in &self.nodes {
            if node == root && entry.parent.is_some() {
                return Err(TreeInvariantError::RootHasParent);
            }
            if node != root && entry.parent.is_none() {
                return Err(TreeInvariantError::MissingParent);
            }
            if let Some(parent) = entry.parent {
                let parent_entry = self.nodes.get(&parent);
                let parent_entry = parent_entry.ok_or(TreeInvariantError::MissingParent)?;
                if !parent_entry.children.contains(&node) {
                    return Err(TreeInvariantError::ParentChildMismatch);
                }
            }
            let mut unique = HashSet::new();
            for &child in &entry.children {
                if !unique.insert(child) {
                    return Err(TreeInvariantError::DuplicateChild);
                }
                if child == root {
                    return Err(TreeInvariantError::RootHasParentReference);
                }
                let child_entry = self.nodes.get(&child);
                let child_entry = child_entry.ok_or(TreeInvariantError::MissingChild)?;
                if child_entry.parent != Some(node) {
                    return Err(TreeInvariantError::ParentChildMismatch);
                }
            }
        }
        let mut visited = HashSet::new();
        self.validate_reachable(root, &mut visited, &mut HashSet::new())?;
        if visited.len() != self.nodes.len() {
            return Err(TreeInvariantError::DisconnectedNode);
        }
        Ok(())
    }

    fn collect_all(&self, node: N, destroyed: &mut Vec<N>) {
        destroyed.push(node);
        for child in self.children(node) {
            self.collect_all(child, destroyed);
        }
    }

    // `destroying_to` is the nearest surviving ancestor for the active cascade.
    fn plan_visit<F>(
        &self,
        node: N,
        destroying_to: Option<N>,
        targets: &HashSet<N>,
        is_independent: &F,
        outcome: &mut DestructionOutcome<N>,
    ) where
        F: Fn(N, &M) -> bool,
    {
        let explicit = targets.contains(&node);
        if let Some(surviving_parent) = destroying_to {
            if !explicit && is_independent(node, &self.nodes[&node].metadata) {
                let from = self.nodes[&node].parent.expect("non-root boundary");
                outcome.reparented.push(Reparented {
                    node,
                    from,
                    to: surviving_parent,
                });
                // A boundary preserves ordinary descendants, but never masks an explicit target.
                for child in self.children(node) {
                    self.plan_visit(child, None, targets, is_independent, outcome);
                }
                return;
            }
            outcome.destroyed.push(node);
            for child in self.children(node) {
                self.plan_visit(
                    child,
                    Some(surviving_parent),
                    targets,
                    is_independent,
                    outcome,
                );
            }
        } else if explicit {
            let parent = self.nodes[&node]
                .parent
                .expect("root target handled separately");
            outcome.destroyed.push(node);
            for child in self.children(node) {
                self.plan_visit(child, Some(parent), targets, is_independent, outcome);
            }
        } else {
            for child in self.children(node) {
                self.plan_visit(child, None, targets, is_independent, outcome);
            }
        }
    }

    fn validate_reachable(
        &self,
        node: N,
        visited: &mut HashSet<N>,
        active: &mut HashSet<N>,
    ) -> Result<(), TreeInvariantError> {
        if !active.insert(node) {
            return Err(TreeInvariantError::Cycle);
        }
        if !visited.insert(node) {
            return Err(TreeInvariantError::Cycle);
        }
        for child in self.children(node) {
            self.validate_reachable(child, visited, active)?;
        }
        let removed = active.remove(&node);
        debug_assert!(removed, "reachable node must be active during traversal");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> AnchorTree<u8, bool> {
        let mut tree = AnchorTree::new(0, false);
        tree.insert_child(0, 1, false).unwrap();
        tree.insert_child(1, 2, false).unwrap();
        tree.insert_child(2, 3, true).unwrap();
        tree.insert_child(3, 4, false).unwrap();
        tree
    }

    #[test]
    fn insertion_and_queries_preserve_order() {
        let mut tree = AnchorTree::new(0, ());
        tree.insert_child(0, 1, ()).unwrap();
        tree.insert_child(0, 2, ()).unwrap();
        assert_eq!(tree.root(), Some(0));
        assert_eq!(tree.children(0).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(tree.parent(2), Some(0));
        assert!(tree.contains(1));
        assert_eq!(tree.insert_child(9, 3, ()), Err(TreeError::UnknownParent));
        assert_eq!(tree.insert_child(0, 0, ()), Err(TreeError::SelfParent));
        assert_eq!(tree.insert_child(0, 1, ()), Err(TreeError::DuplicateNode));
        tree.validate().unwrap();
    }

    #[test]
    fn ordinary_parent_destruction_and_deep_boundary() {
        let mut tree = tree();
        let plan = tree
            .plan_destruction([1], false, |_, independent| *independent)
            .unwrap();
        assert_eq!(plan.destroyed(), &[1, 2]);
        assert_eq!(
            plan.reparented(),
            &[Reparented {
                node: 3,
                from: 2,
                to: 0
            }]
        );
        let outcome = tree.apply_plan(plan).unwrap();
        assert_eq!(outcome.destroyed, vec![1, 2]);
        assert_eq!(tree.parent(3), Some(0));
        assert_eq!(tree.parent(4), Some(3));
        tree.validate().unwrap();
    }

    #[test]
    fn explicit_independent_target_dies_and_overlaps_are_deduplicated() {
        let mut tree = tree();
        let plan = tree
            .plan_destruction([1, 1, 3], false, |_, independent| *independent)
            .unwrap();
        assert_eq!(plan.destroyed(), &[1, 2, 3, 4]);
        tree.apply_plan(plan).unwrap();
        assert_eq!(tree.children(0).count(), 0);
    }

    #[test]
    fn explicit_target_below_boundary_still_dies() {
        let mut tree = tree();
        tree.insert_child(3, 5, false).unwrap();
        let plan = tree
            .plan_destruction([1, 4], false, |_, independent| *independent)
            .unwrap();
        assert_eq!(plan.destroyed(), &[1, 2, 4]);
        assert_eq!(
            plan.reparented(),
            &[Reparented {
                node: 3,
                from: 2,
                to: 0
            }]
        );
        tree.apply_plan(plan).unwrap();
        assert_eq!(tree.parent(3), Some(0));
        assert_eq!(tree.parent(5), Some(3));
        assert!(!tree.contains(4));
    }

    #[test]
    fn explicitly_targeted_survival_boundary_is_destroyed() {
        let mut tree = tree();
        tree.insert_child(3, 5, true).unwrap();
        tree.insert_child(5, 6, false).unwrap();
        let plan = tree
            .plan_destruction([3], false, |_, independent| *independent)
            .unwrap();
        assert_eq!(plan.destroyed(), &[3, 4]);
        assert_eq!(
            plan.reparented(),
            &[Reparented {
                node: 5,
                from: 3,
                to: 2,
            }]
        );
        tree.apply_plan(plan).unwrap();
        assert!(!tree.contains(3));
        assert_eq!(tree.parent(5), Some(2));
        assert_eq!(tree.parent(6), Some(5));
    }

    #[test]
    fn root_requires_unrestricted_mode_and_ignores_independence() {
        let mut tree = tree();
        assert_eq!(
            tree.plan_destruction([0], false, |_, flag| *flag),
            Err(TreeError::RootDestructionRestricted)
        );
        let plan = tree.plan_destruction([0], true, |_, flag| *flag).unwrap();
        assert_eq!(plan.destroyed(), &[0, 1, 2, 3, 4]);
        tree.apply_plan(plan).unwrap();
        assert_eq!(tree.root(), None);
        tree.validate().unwrap();
    }

    #[test]
    fn failed_or_stale_plan_never_mutates_tree() {
        let mut tree = tree();
        let before = tree.children(0).collect::<Vec<_>>();
        assert_eq!(
            tree.plan_destruction([99], false, |_, flag| *flag),
            Err(TreeError::UnknownTarget)
        );
        assert_eq!(tree.children(0).collect::<Vec<_>>(), before);
        let plan = tree.plan_destruction([1], false, |_, flag| *flag).unwrap();
        tree.insert_child(0, 9, false).unwrap();
        assert_eq!(tree.apply_plan(plan), Err(TreeError::PlanDoesNotMatchTree));
        assert!(tree.contains(1));
        tree.validate().unwrap();
    }

    #[test]
    fn multiple_branches_keep_each_boundary() {
        let mut tree = AnchorTree::new(0, false);
        tree.insert_child(0, 1, false).unwrap();
        tree.insert_child(1, 2, true).unwrap();
        tree.insert_child(1, 3, false).unwrap();
        tree.insert_child(3, 4, true).unwrap();
        let plan = tree.plan_destruction([1], false, |_, flag| *flag).unwrap();
        assert_eq!(plan.destroyed(), &[1, 3]);
        assert_eq!(
            plan.reparented(),
            &[
                Reparented {
                    node: 2,
                    from: 1,
                    to: 0
                },
                Reparented {
                    node: 4,
                    from: 3,
                    to: 0
                },
            ]
        );
        tree.apply_plan(plan).unwrap();
        assert_eq!(tree.children(0).collect::<Vec<_>>(), vec![2, 4]);
    }
}
