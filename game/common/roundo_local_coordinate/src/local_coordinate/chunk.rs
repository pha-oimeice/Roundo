use crate::local_coordinate::data::{
    AtomicVoxel, AtomicVoxelId, CHUNK_EDGE_LENGTH, Chunk, EMPTY_VOXEL_ID, LocalAtomicVoxelData,
    SOLID_VOXEL_ID, VoxelTriangle,
};
use crate::local_coordinate::derived_svo::SvoSource;
use bevy::prelude::{IVec3, Vec3};
use roundo_algorithm::tree::BreadthFirstLosslessSvo;
use roundo_algorithm::tree::Node;
use std::sync::Arc;

const CHUNK_OCTREE_DEPTH: usize = CHUNK_EDGE_LENGTH.ilog2() as usize;

impl Chunk {
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
            let index = dense_voxel_index(local_position);
            Arc::make_mut(
                self.primitive_voxels
                    .get_or_insert_with(empty_primitive_voxels),
            )[index] = voxel;
        }

        changed
    }

    /// Returns the primitive data stored at one chunk-relative voxel position.
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

    pub fn is_solid(&self, local_position: IVec3) -> bool {
        self.voxel(local_position).is_some_and(is_solid)
    }

    pub fn is_empty(&self) -> bool {
        self.solid_count == 0
    }

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
                        local_atomic_voxel_data.insert(voxel, LocalAtomicVoxelData);
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

    pub(crate) fn rebuild_triangles(
        &mut self,
        chunk_position: IVec3,
        color_for_voxel: impl Fn(IVec3) -> [f32; 4],
        neighbor_chunk_is_solid: impl Fn(IVec3) -> bool,
    ) {
        self.triangles.clear();

        let chunk_origin = chunk_position * CHUNK_EDGE_LENGTH;
        for z in 0..CHUNK_EDGE_LENGTH {
            for y in 0..CHUNK_EDGE_LENGTH {
                for x in 0..CHUNK_EDGE_LENGTH {
                    let local_position = IVec3::new(x, y, z);
                    if !self.is_solid(local_position) {
                        continue;
                    }

                    let voxel_position = chunk_origin + local_position;
                    let color = color_for_voxel(voxel_position);
                    for face in FaceDirection::ALL {
                        let neighbor_local_position = local_position + face.offset();
                        let neighbor_is_solid = if is_local_position(neighbor_local_position) {
                            self.is_solid(neighbor_local_position)
                        } else {
                            neighbor_chunk_is_solid(voxel_position + face.offset())
                        };

                        if !neighbor_is_solid {
                            self.push_face(local_position, face, color);
                        }
                    }
                }
            }
        }
    }

    fn insert_solid(&mut self, local_position: IVec3, voxel: AtomicVoxel) -> bool {
        let (changed, inserted_new_solid) = {
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
            let inserted_new_solid = changed && node.data == EMPTY_VOXEL_ID;
            if changed {
                node.data = voxel;
            }

            (changed, inserted_new_solid)
        };

        if inserted_new_solid {
            self.solid_count += 1;
        }
        if changed {
            self.local_atomic_voxel_data
                .entry(voxel)
                .or_insert(LocalAtomicVoxelData);
        }

        changed
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
        let mut primitive_voxels = vec![EMPTY_VOXEL_ID; CHUNK_EDGE_LENGTH.pow(3) as usize];
        for (position, voxel) in solids {
            self.insert_solid(position, voxel);
            primitive_voxels[dense_voxel_index(position)] = voxel;
        }
        self.primitive_voxels = Some(primitive_voxels.into());
    }

    fn remove_solid(&mut self, local_position: IVec3) -> bool {
        let removed = remove_from_node(&mut self.octree.unoptimized_octree.root, local_position, 0);

        if removed {
            self.solid_count -= 1;
            if self.solid_count == 0 {
                self.local_atomic_voxel_data.remove(&SOLID_VOXEL_ID);
            }
        }

        removed
    }

    fn push_face(&mut self, voxel_position: IVec3, face: FaceDirection, color: [f32; 4]) {
        let corners = face.corners(voxel_position);
        let normal = face.normal();

        self.triangles.push(VoxelTriangle {
            vertices: [corners[0], corners[1], corners[2]],
            normal,
            color,
        });
        self.triangles.push(VoxelTriangle {
            vertices: [corners[0], corners[2], corners[3]],
            normal,
            color,
        });
    }
}

fn remove_from_node(node: &mut Node<AtomicVoxel, 8>, local_position: IVec3, depth: usize) -> bool {
    let octant = octant_at(local_position, depth);
    let removed = {
        let Some(child) = node.children[octant].as_deref_mut() else {
            return false;
        };

        if depth + 1 == CHUNK_OCTREE_DEPTH {
            let removed = is_solid(child.data);
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
    voxel == SOLID_VOXEL_ID
}

fn octant_at(position: IVec3, depth: usize) -> usize {
    let bit = CHUNK_OCTREE_DEPTH - depth - 1;
    (((position.x >> bit) & 1)
        | (((position.y >> bit) & 1) << 1)
        | (((position.z >> bit) & 1) << 2)) as usize
}

#[derive(Clone, Copy)]
enum FaceDirection {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl FaceDirection {
    const ALL: [Self; 6] = [
        Self::PosX,
        Self::NegX,
        Self::PosY,
        Self::NegY,
        Self::PosZ,
        Self::NegZ,
    ];

    fn offset(self) -> IVec3 {
        match self {
            Self::PosX => IVec3::X,
            Self::NegX => IVec3::NEG_X,
            Self::PosY => IVec3::Y,
            Self::NegY => IVec3::NEG_Y,
            Self::PosZ => IVec3::Z,
            Self::NegZ => IVec3::NEG_Z,
        }
    }

    fn normal(self) -> Vec3 {
        self.offset().as_vec3()
    }

    fn corners(self, position: IVec3) -> [Vec3; 4] {
        let x = position.x as f32;
        let y = position.y as f32;
        let z = position.z as f32;

        match self {
            Self::PosX => [
                Vec3::new(x + 1.0, y, z),
                Vec3::new(x + 1.0, y + 1.0, z),
                Vec3::new(x + 1.0, y + 1.0, z + 1.0),
                Vec3::new(x + 1.0, y, z + 1.0),
            ],
            Self::NegX => [
                Vec3::new(x, y, z),
                Vec3::new(x, y, z + 1.0),
                Vec3::new(x, y + 1.0, z + 1.0),
                Vec3::new(x, y + 1.0, z),
            ],
            Self::PosY => [
                Vec3::new(x, y + 1.0, z),
                Vec3::new(x, y + 1.0, z + 1.0),
                Vec3::new(x + 1.0, y + 1.0, z + 1.0),
                Vec3::new(x + 1.0, y + 1.0, z),
            ],
            Self::NegY => [
                Vec3::new(x, y, z),
                Vec3::new(x + 1.0, y, z),
                Vec3::new(x + 1.0, y, z + 1.0),
                Vec3::new(x, y, z + 1.0),
            ],
            Self::PosZ => [
                Vec3::new(x, y, z + 1.0),
                Vec3::new(x + 1.0, y, z + 1.0),
                Vec3::new(x + 1.0, y + 1.0, z + 1.0),
                Vec3::new(x, y + 1.0, z + 1.0),
            ],
            Self::NegZ => [
                Vec3::new(x, y, z),
                Vec3::new(x, y + 1.0, z),
                Vec3::new(x + 1.0, y + 1.0, z),
                Vec3::new(x + 1.0, y, z),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert!(read_only.set_voxel(IVec3::new(7, 8, 9), SOLID_VOXEL_ID));
        assert!(read_only.octree.len() > 1);
        assert_eq!(read_only.voxel(position), Some(SOLID_VOXEL_ID));
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
