use crate::local_coordinate::data::{AtomicVoxelData, CHUNK_EDGE_LENGTH, Chunk, VoxelTriangle};
use bevy::prelude::{IVec3, Vec3};
use roundo_algorithm::tree::Node;

const CHUNK_OCTREE_DEPTH: usize = CHUNK_EDGE_LENGTH.ilog2() as usize;

impl Chunk {
    pub fn set_voxel(&mut self, local_position: IVec3, data: AtomicVoxelData) -> bool {
        if !is_local_position(local_position) {
            return false;
        }

        let changed = if data.is_solid() {
            self.insert_solid(local_position, data)
        } else {
            self.remove_solid(local_position)
        };

        if changed {
            self.octree.optimized_octree = None;
        }

        changed
    }

    pub fn is_solid(&self, local_position: IVec3) -> bool {
        if !is_local_position(local_position) {
            return false;
        }

        let mut node = &self.octree.unoptimized_octree.root;
        for depth in 0..CHUNK_OCTREE_DEPTH {
            let octant = octant_at(local_position, depth);
            let Some(child) = node.children[octant].as_deref() else {
                return false;
            };
            node = child;
        }

        node.data.is_some_and(AtomicVoxelData::is_solid)
    }

    pub fn is_empty(&self) -> bool {
        self.solid_count == 0
    }

    pub(crate) fn rebuild_triangles(
        &mut self,
        chunk_position: IVec3,
        is_solid_outside_chunk: impl Fn(IVec3) -> bool,
        color_for_voxel: impl Fn(IVec3) -> [f32; 4],
    ) {
        self.triangles.clear();
        self.triangles.reserve(self.solid_count * 12);

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
                            is_solid_outside_chunk(chunk_origin + neighbor_local_position)
                        };

                        if !neighbor_is_solid {
                            self.push_face(voxel_position, face, color);
                        }
                    }
                }
            }
        }
    }

    fn insert_solid(&mut self, local_position: IVec3, data: AtomicVoxelData) -> bool {
        let (changed, inserted_new_solid) = {
            let mut node = &mut self.octree.unoptimized_octree.root;
            let mut node_id = 0_usize;

            for depth in 0..CHUNK_OCTREE_DEPTH {
                let octant = octant_at(local_position, depth);
                node_id = node_id * 8 + octant + 1;
                if node.children[octant].is_none() {
                    node.children[octant] = Some(Box::new(Node::new(node_id as u32, None)));
                }
                node = node.children[octant]
                    .as_deref_mut()
                    .expect("newly inserted octree path node must exist");
            }

            let changed = node.data != Some(data);
            let inserted_new_solid = changed && node.data.is_none();
            if changed {
                node.data = Some(data);
            }

            (changed, inserted_new_solid)
        };

        if inserted_new_solid {
            self.solid_count += 1;
        }

        changed
    }

    fn remove_solid(&mut self, local_position: IVec3) -> bool {
        let removed = remove_from_node(&mut self.octree.unoptimized_octree.root, local_position, 0);

        if removed {
            self.solid_count -= 1;
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

fn remove_from_node(
    node: &mut Node<Option<AtomicVoxelData>, 8>,
    local_position: IVec3,
    depth: usize,
) -> bool {
    let octant = octant_at(local_position, depth);
    let removed = {
        let Some(child) = node.children[octant].as_deref_mut() else {
            return false;
        };

        if depth + 1 == CHUNK_OCTREE_DEPTH {
            child.data.take().is_some_and(AtomicVoxelData::is_solid)
        } else {
            remove_from_node(child, local_position, depth + 1)
        }
    };

    if node.children[octant].as_deref().is_some_and(is_empty_node) {
        node.children[octant] = None;
    }

    removed
}

fn is_empty_node(node: &Node<Option<AtomicVoxelData>, 8>) -> bool {
    node.data.is_none() && node.children.iter().all(Option::is_none)
}

fn is_local_position(position: IVec3) -> bool {
    (0..CHUNK_EDGE_LENGTH).contains(&position.x)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.y)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.z)
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
