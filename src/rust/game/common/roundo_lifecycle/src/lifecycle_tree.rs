//! UI ownership and lifetime semantics over the generic ordered-tree algorithm.

use roundo_dsa::data_structure::graph::{
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
