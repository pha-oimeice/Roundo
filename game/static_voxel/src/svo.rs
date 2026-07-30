use crate::{CHUNK_EDGE_LENGTH, EMPTY_MATERIAL_ID, GeneratedChunk};
use roundo_algorithm::tree::{
    CompressionSettings, Node, OptimizedNode, OptimizedOctree, UnoptimizedOctree,
};

const CHUNK_OCTREE_DEPTH: usize = CHUNK_EDGE_LENGTH.ilog2() as usize;

/// Client-owned immutable SVO cache. LOD zero is the finest leaf level.
pub struct StaticVoxelSvoView {
    tree: OptimizedOctree<Option<u16>>,
}

impl StaticVoxelSvoView {
    pub fn from_chunk(chunk: &GeneratedChunk) -> Self {
        let mut source = UnoptimizedOctree::new(0, None);

        for z in 0..CHUNK_EDGE_LENGTH {
            for y in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let material = chunk
                        .voxel([x, y, z])
                        .expect("local chunk position must be in bounds");
                    if material == EMPTY_MATERIAL_ID {
                        continue;
                    }
                    insert_voxel(&mut source.root, [x, y, z], material);
                }
            }
        }

        let tree = OptimizedOctree::from_unoptimized_with_settings(
            &source,
            CompressionSettings::full_solid(),
        )
        .expect("a fixed 16-cubed chunk fits the compact SVO index range");
        Self { tree }
    }

    pub fn maximum_lod(&self) -> usize {
        self.tree.level_offsets.len().saturating_sub(2)
    }

    /// Reads one packed SVO level; higher LOD values are progressively coarser.
    pub fn nodes_for_lod(&self, lod: usize) -> &[OptimizedNode<Option<u16>>] {
        let maximum_depth = self.maximum_lod();
        let depth = maximum_depth.saturating_sub(lod.min(maximum_depth));
        let start = self.tree.level_offsets[depth] as usize;
        let end = self.tree.level_offsets[depth + 1] as usize;
        &self.tree.nodes[start..end]
    }

    pub fn tree(&self) -> &OptimizedOctree<Option<u16>> {
        &self.tree
    }
}

fn insert_voxel(root: &mut Node<Option<u16>, 8>, position: [usize; 3], material: u16) {
    let mut node = root;
    let mut node_id = 0_usize;

    for depth in 0..CHUNK_OCTREE_DEPTH {
        let bit = CHUNK_OCTREE_DEPTH - depth - 1;
        let octant = ((position[0] >> bit) & 1)
            | (((position[1] >> bit) & 1) << 1)
            | (((position[2] >> bit) & 1) << 2);
        node_id = node_id * 8 + octant + 1;
        node =
            node.children[octant].get_or_insert_with(|| Box::new(Node::new(node_id as u32, None)));
    }
    node.data = Some(material);
}
