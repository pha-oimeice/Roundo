//! Chunk-local voxel lookup, mutation, and immutable SVO snapshots.

use crate::local_coordinate::data::{
    AtomicVoxel, AtomicVoxelId, CHUNK_EDGE_LENGTH, Chunk, EMPTY_VOXEL_ID, LocalAtomicVoxelData,
};
use crate::local_coordinate::derived_svo::SvoSource;
use bevy::prelude::IVec3;
use roundo_algorithm::tree::BreadthFirstLosslessSvo;
use roundo_algorithm::tree::Node;
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
        self.materialize_read_only_svo();

        let changed = if is_solid(voxel) {
            self.insert_solid(local_position, voxel)
        } else {
            self.remove_solid(local_position)
        };

        if changed {
            self.octree.optimized_octree = None;
            self.content_revision = self.content_revision.wrapping_add(1);
            let index = dense_voxel_index(local_position);
            Arc::make_mut(
                self.primitive_voxels
                    .get_or_insert_with(empty_primitive_voxels),
            )[index] = voxel;
        }

        changed
    }

    /// Returns the nonempty voxel at a chunk-local integer position.
    ///
    /// `None` means either that the position is outside the chunk or that it
    /// contains [`EMPTY_VOXEL_ID`]. This lookup does not materialize an editable
    /// octree when the chunk is backed by a read-only SVO.
    pub fn voxel(&self, local_position: IVec3) -> Option<AtomicVoxel> {
        if !is_local_position(local_position) {
            return None;
        }

        if let Some(view) = &self.read_only_svo {
            return view
                .value_at_coordinates(local_position.as_uvec3().to_array())
                .copied()
                .and_then(|voxel| (voxel != EMPTY_VOXEL_ID).then_some(voxel));
        }

        let mut node = &self.octree.unoptimized_octree.root;
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

    /// Creates a chunk whose supplied SVO is authoritative until first mutation.
    ///
    /// Construction scans the full chunk to derive occupancy counts but does not
    /// materialize editable octree nodes or a dense primitive snapshot.
    pub(crate) fn from_read_only_svo(view: Arc<BreadthFirstLosslessSvo<AtomicVoxel>>) -> Self {
        let mut solid_count = 0;
        let mut local_atomic_voxel_data = std::collections::HashMap::new();
        for z in 0..CHUNK_EDGE_LENGTH {
            for y in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let position = [x as u32, y as u32, z as u32];
                    let voxel = *view
                        .value_at_coordinates(position)
                        .expect("chunk coordinates fit the SVO depth");
                    if voxel != EMPTY_VOXEL_ID {
                        solid_count += 1;
                        local_atomic_voxel_data
                            .entry(voxel)
                            .or_insert_with(LocalAtomicVoxelData::default)
                            .occurrences += 1;
                    }
                }
            }
        }
        Self {
            primitive_voxels: None,
            read_only_svo: Some(view),
            read_only_svo_is_authoritative: true,
            local_atomic_voxel_data,
            solid_count,
            ..Default::default()
        }
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
                &self.octree.unoptimized_octree,
                CHUNK_OCTREE_DEPTH as u8,
                |voxel| *voxel,
            )
            .expect("a fixed-size chunk SVO fits in compact indices"),
        );
        self.read_only_svo = Some(Arc::clone(&view));
        self.read_only_svo_is_authoritative = false;
        view
    }

    /// Captures immutable source data for off-thread SVO derivation.
    ///
    /// The returned `Arc` owns a point-in-time snapshot and remains valid after
    /// subsequent copy-on-write mutation of this chunk.
    pub(crate) fn svo_source(&self) -> SvoSource {
        self.read_only_svo.as_ref().map_or_else(
            || {
                SvoSource::Primitive(Arc::clone(
                    self.primitive_voxels
                        .as_ref()
                        .expect("editable chunks retain a primitive voxel snapshot"),
                ))
            },
            |svo| SvoSource::Cached(Arc::clone(svo)),
        )
    }

    fn insert_solid(&mut self, local_position: IVec3, voxel: AtomicVoxel) -> bool {
        let (changed, previous) = {
            let mut node = &mut self.octree.unoptimized_octree.root;
            let mut node_id = 0_usize;

            for depth in 0..CHUNK_OCTREE_DEPTH {
                let octant = octant_at(local_position, depth);
                node_id = node_id * 8 + octant + 1;
                if node.children[octant].is_none() {
                    node.children[octant] =
                        Some(Box::new(Node::new(node_id as u32, EMPTY_VOXEL_ID)));
                }
                node = node.children[octant]
                    .as_deref_mut()
                    .expect("newly inserted octree path node must exist");
            }

            let changed = node.data != voxel;
            let previous = node.data;
            if changed {
                node.data = voxel;
            }

            (changed, previous)
        };

        if !changed {
            return false;
        }
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

    fn materialize_read_only_svo(&mut self) {
        let Some(view) = self.read_only_svo.take() else {
            return;
        };
        if !self.read_only_svo_is_authoritative {
            return;
        }
        self.read_only_svo_is_authoritative = false;
        let mut solids = Vec::with_capacity(self.solid_count);
        for z in 0..CHUNK_EDGE_LENGTH {
            for y in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let position = IVec3::new(x, y, z);
                    let voxel = *view
                        .value_at_coordinates(position.as_uvec3().to_array())
                        .expect("chunk coordinates fit the SVO depth");
                    if voxel != EMPTY_VOXEL_ID {
                        solids.push((position, voxel));
                    }
                }
            }
        }

        self.octree = roundo_algorithm::tree::Octree::new(0, EMPTY_VOXEL_ID);
        self.solid_count = 0;
        self.local_atomic_voxel_data.clear();
        let mut primitive_voxels = vec![EMPTY_VOXEL_ID; CHUNK_EDGE_LENGTH.pow(3) as usize];
        for (position, voxel) in solids {
            self.insert_solid(position, voxel);
            primitive_voxels[dense_voxel_index(position)] = voxel;
        }
        self.primitive_voxels = Some(primitive_voxels.into());
    }

    fn remove_solid(&mut self, local_position: IVec3) -> bool {
        let Some(removed) =
            remove_from_node(&mut self.octree.unoptimized_octree.root, local_position, 0)
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

fn remove_from_node(
    node: &mut Node<AtomicVoxel, 8>,
    local_position: IVec3,
    depth: usize,
) -> Option<AtomicVoxel> {
    let octant = octant_at(local_position, depth);
    let removed = {
        let child = node.children[octant].as_deref_mut()?;

        if depth + 1 == CHUNK_OCTREE_DEPTH {
            let removed = is_solid(child.data).then_some(child.data);
            child.data = EMPTY_VOXEL_ID;
            removed
        } else {
            remove_from_node(child, local_position, depth + 1)
        }
    };

    if node.children[octant].as_deref().is_some_and(is_empty_node) {
        node.children[octant] = None;
    }

    removed
}

fn is_empty_node(node: &Node<AtomicVoxel, 8>) -> bool {
    node.data == EMPTY_VOXEL_ID && node.children.iter().all(Option::is_none)
}

fn is_local_position(position: IVec3) -> bool {
    (0..CHUNK_EDGE_LENGTH).contains(&position.x)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.y)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.z)
}

fn dense_voxel_index(position: IVec3) -> usize {
    position.x as usize
        + position.y as usize * CHUNK_EDGE_LENGTH as usize
        + position.z as usize * CHUNK_EDGE_LENGTH.pow(2) as usize
}

fn empty_primitive_voxels() -> Arc<[AtomicVoxel]> {
    vec![EMPTY_VOXEL_ID; CHUNK_EDGE_LENGTH.pow(3) as usize].into()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SOLID_VOXEL_ID;

    #[test]
    fn voxel_updates_report_whether_data_changed() {
        let mut chunk = Chunk::default();
        let solid = SOLID_VOXEL_ID;

        assert!(chunk.set_voxel(IVec3::new(1, 2, 3), solid));
        assert!(chunk.local_atomic_voxel_data.contains_key(&solid));
        assert!(!chunk.set_voxel(IVec3::new(1, 2, 3), solid));
        assert!(chunk.set_voxel(IVec3::new(1, 2, 3), EMPTY_VOXEL_ID));
        assert!(!chunk.local_atomic_voxel_data.contains_key(&solid));
    }

    #[test]
    fn different_registered_voxel_ids_remain_solid_and_track_local_presence() {
        let mut chunk = Chunk::default();
        let first = AtomicVoxelId(7);
        let second = AtomicVoxelId(9);

        assert!(chunk.set_voxel(IVec3::ZERO, first));
        assert!(chunk.set_voxel(IVec3::ZERO, second));
        assert_eq!(chunk.voxel(IVec3::ZERO), Some(second));
        assert!(!chunk.local_atomic_voxel_data.contains_key(&first));
        assert!(chunk.local_atomic_voxel_data.contains_key(&second));
        assert!(chunk.set_voxel(IVec3::ZERO, EMPTY_VOXEL_ID));
        assert!(chunk.local_atomic_voxel_data.is_empty());
    }

    #[test]
    fn read_only_svo_is_queried_without_primitive_materialization() {
        let mut source = Chunk::default();
        let position = IVec3::new(4, 5, 6);
        assert!(source.set_voxel(position, SOLID_VOXEL_ID));
        let view = source.read_only_svo();
        let mut read_only = Chunk::from_read_only_svo(view);

        assert_eq!(read_only.octree.len(), 1);
        assert_eq!(read_only.voxel(position), Some(SOLID_VOXEL_ID));
        assert!(
            read_only
                .local_atomic_voxel_data
                .contains_key(&SOLID_VOXEL_ID)
        );
        assert_eq!(read_only.octree.len(), 1);

        let second_position = IVec3::new(7, 8, 9);
        assert!(read_only.set_voxel(second_position, SOLID_VOXEL_ID));
        assert!(read_only.octree.len() > 1);
        assert_eq!(read_only.voxel(position), Some(SOLID_VOXEL_ID));
        assert_eq!(
            read_only.local_atomic_voxel_data[&SOLID_VOXEL_ID].occurrences,
            2
        );
        assert!(read_only.set_voxel(position, EMPTY_VOXEL_ID));
        assert!(
            read_only
                .local_atomic_voxel_data
                .contains_key(&SOLID_VOXEL_ID)
        );
    }

    #[test]
    fn derived_svo_source_is_an_immutable_cow_snapshot() {
        let mut chunk = Chunk::default();
        assert!(chunk.set_voxel(IVec3::ZERO, SOLID_VOXEL_ID));
        let snapshot = match chunk.svo_source() {
            SvoSource::Primitive(snapshot) => snapshot,
            SvoSource::Cached(_) => panic!("editable chunk should expose primitive data"),
        };

        assert!(chunk.set_voxel(IVec3::X, SOLID_VOXEL_ID));

        assert_eq!(snapshot[dense_voxel_index(IVec3::ZERO)], SOLID_VOXEL_ID);
        assert_eq!(snapshot[dense_voxel_index(IVec3::X)], EMPTY_VOXEL_ID);
        assert_eq!(
            chunk.primitive_voxels.as_ref().unwrap()[dense_voxel_index(IVec3::X)],
            SOLID_VOXEL_ID
        );
    }
}
