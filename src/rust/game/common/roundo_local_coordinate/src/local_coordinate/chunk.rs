//! Chunk-local voxel lookup, mutation, and immutable SVO snapshots.

use crate::local_coordinate::data::{
    AtomicVoxel, AtomicVoxelId, CHUNK_EDGE_LENGTH, Chunk, EMPTY_VOXEL_ID, LocalAtomicVoxelData,
};
use crate::local_coordinate::derived_svo::SvoSource;
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
        self.materialize_read_only_svo();

        let changed = if is_solid(voxel) {
            self.insert_solid(local_position, voxel)
        } else {
            self.remove_solid(local_position)
        };

        if changed {
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
                self.octree.source(),
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

        self.octree = roundo_dsa::data_structure::tree::FixedDepthOctree::new(
            CHUNK_OCTREE_DEPTH as u8,
            EMPTY_VOXEL_ID,
        )
        .expect("fixed chunk depth fits u32 path node IDs");
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
