//! Chunk-local voxel lookup, mutation, and immutable SVO snapshots.

use crate::local_coordinate::data::{
    AtomicVoxel, AtomicVoxelId, CHUNK_EDGE_LENGTH, Chunk, EMPTY_VOXEL_ID, LocalAtomicVoxelData,
};
use bevy::prelude::IVec3;
use roundo_dsa::data_structure::tree::BreadthFirstLosslessSvo;
use std::sync::Arc;

const CHUNK_OCTREE_DEPTH: usize = CHUNK_EDGE_LENGTH.ilog2() as usize;

impl Chunk {
    /// Replaces one voxel at a chunk-local integer position.
    ///
    /// Each axis must be in `0..CHUNK_EDGE_LENGTH`. Any nonzero ID is treated as
    /// solid; this low-level method does not verify that the ID is registered.
    /// Returns `true` only when voxel content changes. An out-of-bounds position
    /// or an idempotent write returns `false`.
    ///
    /// A content change advances `content_revision` with wrapping arithmetic
    /// and invalidates cached derived SVO state. Existing `Arc`
    /// snapshots remain unchanged.
    pub fn set_voxel(&mut self, local_position: IVec3, voxel: AtomicVoxel) -> bool {
        if !is_local_position(local_position) {
            return false;
        }

        let changed = if is_solid(voxel) {
            self.insert_solid(local_position, voxel)
        } else {
            self.remove_solid(local_position)
        };

        if changed {
            self.content_revision = self.content_revision.wrapping_add(1);
            self.read_only_svo = None;
        }

        changed
    }

    /// Returns the nonempty voxel at a chunk-local integer position.
    ///
    /// `None` means either that the position is outside the chunk or that it
    /// contains [`EMPTY_VOXEL_ID`].
    pub fn voxel(&self, local_position: IVec3) -> Option<AtomicVoxel> {
        if !is_local_position(local_position) {
            return None;
        }

        let mut node = &self.octree.source().root;
        for depth in 0..CHUNK_OCTREE_DEPTH {
            let octant = octant_at(local_position, depth);
            let Some(child) = node.children[octant].as_deref() else {
                return None;
            };
            node = child;
        }

        (node.data != EMPTY_VOXEL_ID).then_some(node.data)
    }

    /// Returns whether an in-bounds position contains a nonzero voxel ID.
    ///
    /// Out-of-bounds positions are reported as non-solid.
    pub fn is_solid(&self, local_position: IVec3) -> bool {
        self.voxel(local_position).is_some_and(is_solid)
    }

    /// Returns whether the chunk contains no nonzero voxel IDs.
    pub fn is_empty(&self) -> bool {
        self.solid_count == 0
    }

    /// Borrows the currently cached compressed representation, if any.
    ///
    /// Editable chunks may have no compressed cache. A successful voxel change
    /// invalidates this entry, while any previously cloned `Arc` remains a
    /// stable snapshot of the older content.
    pub fn compressed_svo(&self) -> Option<&Arc<BreadthFirstLosslessSvo<AtomicVoxel>>> {
        self.read_only_svo.as_ref()
    }

    /// Returns an immutable compressed snapshot, building and caching it if needed.
    ///
    /// Cloned snapshots are not updated by later chunk mutations.
    pub(crate) fn read_only_svo(&mut self) -> Arc<BreadthFirstLosslessSvo<AtomicVoxel>> {
        if let Some(view) = &self.read_only_svo {
            return Arc::clone(view);
        }
        let view = Arc::new(
            BreadthFirstLosslessSvo::from_unoptimized_mapped(
                self.octree.source(),
                CHUNK_OCTREE_DEPTH as u8,
                |voxel| *voxel,
            )
            .expect("a fixed-size chunk SVO fits in compact indices"),
        );
        self.read_only_svo = Some(Arc::clone(&view));
        view
    }

    fn insert_solid(&mut self, local_position: IVec3, voxel: AtomicVoxel) -> bool {
        let Some(previous) = self
            .octree
            .set(&octree_path(local_position), voxel)
            .expect("in-bounds chunk position has a valid fixed-depth path")
        else {
            return false;
        };
        if previous == EMPTY_VOXEL_ID {
            self.solid_count += 1;
        } else {
            self.remove_voxel_occurrence(previous);
        }
        self.local_atomic_voxel_data
            .entry(voxel)
            .or_insert_with(LocalAtomicVoxelData::default)
            .occurrences += 1;
        true
    }

    fn remove_solid(&mut self, local_position: IVec3) -> bool {
        let Some(removed) = self
            .octree
            .set(&octree_path(local_position), EMPTY_VOXEL_ID)
            .expect("in-bounds chunk position has a valid fixed-depth path")
        else {
            return false;
        };
        self.solid_count -= 1;
        self.remove_voxel_occurrence(removed);
        true
    }

    fn remove_voxel_occurrence(&mut self, voxel: AtomicVoxelId) {
        let remove = self
            .local_atomic_voxel_data
            .get_mut(&voxel)
            .is_some_and(|data| {
                data.occurrences -= 1;
                data.occurrences == 0
            });
        if remove {
            let removed = self.local_atomic_voxel_data.remove(&voxel);
            debug_assert!(removed.is_some(), "tracked voxel occurrence must exist");
        }
    }
}

fn octree_path(position: IVec3) -> [u8; CHUNK_OCTREE_DEPTH] {
    std::array::from_fn(|depth| octant_at(position, depth) as u8)
}

fn is_local_position(position: IVec3) -> bool {
    (0..CHUNK_EDGE_LENGTH).contains(&position.x)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.y)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.z)
}

fn is_solid(voxel: AtomicVoxelId) -> bool {
    voxel != EMPTY_VOXEL_ID
}

fn octant_at(position: IVec3, depth: usize) -> usize {
    let bit = CHUNK_OCTREE_DEPTH - depth - 1;
    (((position.x >> bit) & 1)
        | (((position.y >> bit) & 1) << 1)
        | (((position.z >> bit) & 1) << 2)) as usize
}
