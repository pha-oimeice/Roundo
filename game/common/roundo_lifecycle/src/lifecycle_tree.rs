//! UI ownership and lifetime semantics over the generic ordered-tree algorithm.

use roundo_algorithm::graph::{
    OrderedRootedTree, RootedTreeError, RootedTreeInvariantError, TreeReparent,
};
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TREE_ID: AtomicU64 = AtomicU64::new(1);

pub type TreeInvariantError = RootedTreeInvariantError;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reparented<N> {
    pub node: N,
    pub from: N,
    pub to: N,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestructionOutcome<N> {
    pub destroyed: Vec<N>,
    pub reparented: Vec<Reparented<N>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestructionPlan<N> {
    tree_id: u64,
    version: u64,
    outcome: DestructionOutcome<N>,
}

impl<N> DestructionPlan<N> {
    pub fn destroyed(&self) -> &[N] {
        &self.outcome.destroyed
    }

    pub fn reparented(&self) -> &[Reparented<N>] {
        &self.outcome.reparented
    }
}

/// UI lifecycle ownership tree.
///
/// Generic topology, ordering, and invariant checks are delegated to
/// [`OrderedRootedTree`]. This module owns lifecycle independence, destruction
/// planning, stale-plan rejection, and lifecycle-specific errors.
#[derive(Debug)]
pub struct AnchorTree<N, M> {
    topology: OrderedRootedTree<N, M>,
    tree_id: u64,
    version: u64,
}

impl<N, M> AnchorTree<N, M>
where
    N: Copy + Eq + Hash,
{
    pub fn new(root: N, metadata: M) -> Self {
        Self {
            topology: OrderedRootedTree::new(root, metadata),
            tree_id: NEXT_TREE_ID.fetch_add(1, Ordering::Relaxed),
            version: 0,
        }
    }

    pub fn root(&self) -> Option<N> {
        self.topology.root()
    }

    pub fn contains(&self, node: N) -> bool {
        self.topology.contains(node)
    }

    pub fn parent(&self, node: N) -> Option<N> {
        self.topology.parent(node)
    }

    pub fn metadata(&self, node: N) -> Option<&M> {
        self.topology.metadata(node)
    }

    pub fn children(&self, node: N) -> impl Iterator<Item = N> + '_ {
        self.topology.children(node)
    }

    pub fn insert_child(&mut self, parent: N, node: N, metadata: M) -> Result<(), TreeError> {
        self.topology
            .insert_child(parent, node, metadata)
            .map_err(map_insert_error)?;
        self.version += 1;
        Ok(())
    }

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
            if !self.topology.contains(*target) {
                return Err(TreeError::UnknownTarget);
            }
        }
        let Some(root) = self.topology.root() else {
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

    pub fn apply_plan(
        &mut self,
        plan: DestructionPlan<N>,
    ) -> Result<DestructionOutcome<N>, TreeError> {
        if plan.tree_id != self.tree_id || plan.version != self.version {
            return Err(TreeError::PlanDoesNotMatchTree);
        }
        self.validate().map_err(TreeError::InvalidTree)?;
        let outcome = plan.outcome;
        let edits = outcome
            .reparented
            .iter()
            .map(|change| TreeReparent {
                node: change.node,
                from: change.from,
                to: change.to,
            })
            .collect::<Vec<_>>();
        self.topology
            .apply_edits(&edits, &outcome.destroyed)
            .map_err(|_| TreeError::PlanDoesNotMatchTree)?;
        self.version += 1;
        Ok(outcome)
    }

    pub fn validate(&self) -> Result<(), TreeInvariantError> {
        self.topology.validate()
    }

    fn collect_all(&self, node: N, destroyed: &mut Vec<N>) {
        destroyed.push(node);
        for child in self.children(node) {
            self.collect_all(child, destroyed);
        }
    }

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
            if !explicit
                && is_independent(
                    node,
                    self.topology.metadata(node).expect("planned node is live"),
                )
            {
                let from = self.topology.parent(node).expect("non-root boundary");
                outcome.reparented.push(Reparented {
                    node,
                    from,
                    to: surviving_parent,
                });
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
            let parent = self
                .topology
                .parent(node)
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
}

fn map_insert_error(error: RootedTreeError) -> TreeError {
    match error {
        RootedTreeError::UnknownParent => TreeError::UnknownParent,
        RootedTreeError::DuplicateNode => TreeError::DuplicateNode,
        RootedTreeError::SelfParent => TreeError::SelfParent,
        RootedTreeError::InvalidStructure(error) => TreeError::InvalidTree(error),
        RootedTreeError::UnknownNode | RootedTreeError::InvalidReparent => {
            TreeError::PlanDoesNotMatchTree
        }
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
