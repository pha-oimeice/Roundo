use crate::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, EMPTY_MATERIAL_ID, GeneratedChunk, StaticVoxelSvoView,
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::{Mesh, Vec3},
    render::render_resource::PrimitiveTopology,
    tasks::AsyncComputeTaskPool,
};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};

pub(crate) const FACE_OFFSETS: [ChunkCoordinate; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

pub(crate) struct ChunkMeshRequest {
    pub coordinate: ChunkCoordinate,
    pub revision: u64,
    pub center: GeneratedChunk,
    pub neighbors: [Option<GeneratedChunk>; 6],
}

pub(crate) struct ChunkSvoRequest {
    pub coordinate: ChunkCoordinate,
    pub revision: u64,
    pub chunk: GeneratedChunk,
}

pub(crate) struct ChunkMeshResult {
    pub coordinate: ChunkCoordinate,
    pub revision: u64,
    pub mesh: Option<ChunkMeshData>,
}

pub(crate) struct ChunkSvoResult {
    pub coordinate: ChunkCoordinate,
    pub revision: u64,
    pub svo: StaticVoxelSvoView,
}

pub(crate) struct ChunkMeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl ChunkMeshData {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn into_mesh(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        mesh.insert_indices(Indices::U32(self.indices));
        mesh
    }
}

pub(crate) struct StaticVoxelDerivationPipeline {
    mesh_sender: CrossbeamThreadPipeEndpointB<(), ChunkMeshResult>,
    mesh_receiver: CrossbeamThreadPipeEndpointA<(), ChunkMeshResult>,
    svo_sender: CrossbeamThreadPipeEndpointB<(), ChunkSvoResult>,
    svo_receiver: CrossbeamThreadPipeEndpointA<(), ChunkSvoResult>,
    mesh_jobs_in_flight: usize,
    svo_jobs_in_flight: usize,
    maximum_mesh_jobs_in_flight: usize,
}

impl StaticVoxelDerivationPipeline {
    pub fn new() -> Self {
        let mesh_pipe = CrossbeamThreadPipe::<(), ChunkMeshResult>::new();
        let svo_pipe = CrossbeamThreadPipe::<(), ChunkSvoResult>::new();
        let maximum_mesh_jobs_in_flight = AsyncComputeTaskPool::get()
            .thread_num()
            .saturating_mul(4)
            .max(1);
        Self {
            mesh_sender: mesh_pipe.endpoint_b(),
            mesh_receiver: mesh_pipe.endpoint_a(),
            svo_sender: svo_pipe.endpoint_b(),
            svo_receiver: svo_pipe.endpoint_a(),
            mesh_jobs_in_flight: 0,
            svo_jobs_in_flight: 0,
            maximum_mesh_jobs_in_flight,
        }
    }

    pub fn available_mesh_job_slots(&self) -> usize {
        self.maximum_mesh_jobs_in_flight
            .saturating_sub(self.mesh_jobs_in_flight)
    }

    pub fn has_mesh_jobs_in_flight(&self) -> bool {
        self.mesh_jobs_in_flight != 0
    }

    pub fn can_start_svo_job(&self) -> bool {
        self.svo_jobs_in_flight == 0
    }

    pub fn start_mesh_job(&mut self, request: ChunkMeshRequest) -> bool {
        if self.available_mesh_job_slots() == 0 {
            return false;
        }
        self.mesh_jobs_in_flight += 1;
        let sender = self.mesh_sender.clone();
        AsyncComputeTaskPool::get()
            .spawn(async move {
                let result = ChunkMeshResult {
                    coordinate: request.coordinate,
                    revision: request.revision,
                    mesh: build_chunk_mesh(&request),
                };
                let _ = sender.try_send(result);
            })
            .detach();
        true
    }

    pub fn start_svo_job(&mut self, request: ChunkSvoRequest) -> bool {
        if !self.can_start_svo_job() {
            return false;
        }
        self.svo_jobs_in_flight += 1;
        let sender = self.svo_sender.clone();
        AsyncComputeTaskPool::get()
            .spawn(async move {
                let result = ChunkSvoResult {
                    coordinate: request.coordinate,
                    revision: request.revision,
                    svo: StaticVoxelSvoView::from_chunk(&request.chunk),
                };
                let _ = sender.try_send(result);
            })
            .detach();
        true
    }

    pub fn try_receive_mesh(&mut self) -> Option<ChunkMeshResult> {
        let result = self.mesh_receiver.try_receive()?;
        self.mesh_jobs_in_flight = self.mesh_jobs_in_flight.saturating_sub(1);
        Some(result)
    }

    pub fn try_receive_svo(&mut self) -> Option<ChunkSvoResult> {
        let result = self.svo_receiver.try_receive()?;
        self.svo_jobs_in_flight = self.svo_jobs_in_flight.saturating_sub(1);
        Some(result)
    }
}

fn build_chunk_mesh(request: &ChunkMeshRequest) -> Option<ChunkMeshData> {
    if request.center.is_empty() {
        return None;
    }

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    for z in 0..CHUNK_EDGE_LENGTH {
        for y in 0..CHUNK_EDGE_LENGTH {
            for x in 0..CHUNK_EDGE_LENGTH {
                if request.center.voxel([x, y, z]) == Some(EMPTY_MATERIAL_ID) {
                    continue;
                }
                let local_position = [x as i32, y as i32, z as i32];
                for face in Face::ALL {
                    let neighbor = add_positions(local_position, face.offset());
                    if material_at(request, neighbor) != EMPTY_MATERIAL_ID {
                        continue;
                    }

                    let first_index = u32::try_from(positions.len()).ok()?;
                    for corner in face.corners(local_position) {
                        positions.push(corner.to_array());
                        normals.push(face.normal().to_array());
                    }
                    indices.extend([
                        first_index,
                        first_index + 1,
                        first_index + 2,
                        first_index,
                        first_index + 2,
                        first_index + 3,
                    ]);
                }
            }
        }
    }

    (!positions.is_empty()).then_some(ChunkMeshData {
        positions,
        normals,
        indices,
    })
}

fn material_at(request: &ChunkMeshRequest, position: [i32; 3]) -> u16 {
    let edge = CHUNK_EDGE_LENGTH as i32;
    let neighbor_index = if position[0] >= edge {
        Some(0)
    } else if position[0] < 0 {
        Some(1)
    } else if position[1] >= edge {
        Some(2)
    } else if position[1] < 0 {
        Some(3)
    } else if position[2] >= edge {
        Some(4)
    } else if position[2] < 0 {
        Some(5)
    } else {
        None
    };
    let local = position.map(|value| value.rem_euclid(edge) as usize);
    neighbor_index
        .and_then(|index| request.neighbors[index].as_ref())
        .map_or_else(
            || {
                if neighbor_index.is_some() {
                    EMPTY_MATERIAL_ID
                } else {
                    request.center.voxel(local).unwrap_or(EMPTY_MATERIAL_ID)
                }
            },
            |chunk| chunk.voxel(local).unwrap_or(EMPTY_MATERIAL_ID),
        )
}

fn add_positions(first: [i32; 3], second: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|axis| first[axis] + second[axis])
}

#[derive(Clone, Copy)]
enum Face {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
}

impl Face {
    const ALL: [Self; 6] = [
        Self::PositiveX,
        Self::NegativeX,
        Self::PositiveY,
        Self::NegativeY,
        Self::PositiveZ,
        Self::NegativeZ,
    ];

    fn offset(self) -> [i32; 3] {
        match self {
            Self::PositiveX => [1, 0, 0],
            Self::NegativeX => [-1, 0, 0],
            Self::PositiveY => [0, 1, 0],
            Self::NegativeY => [0, -1, 0],
            Self::PositiveZ => [0, 0, 1],
            Self::NegativeZ => [0, 0, -1],
        }
    }

    fn normal(self) -> Vec3 {
        Vec3::from_array(self.offset().map(|value| value as f32))
    }

    fn corners(self, [x, y, z]: [i32; 3]) -> [Vec3; 4] {
        let [x, y, z] = [x as f32, y as f32, z as f32];
        match self {
            Self::PositiveX => [
                Vec3::new(x + 1.0, y, z),
                Vec3::new(x + 1.0, y + 1.0, z),
                Vec3::new(x + 1.0, y + 1.0, z + 1.0),
                Vec3::new(x + 1.0, y, z + 1.0),
            ],
            Self::NegativeX => [
                Vec3::new(x, y, z),
                Vec3::new(x, y, z + 1.0),
                Vec3::new(x, y + 1.0, z + 1.0),
                Vec3::new(x, y + 1.0, z),
            ],
            Self::PositiveY => [
                Vec3::new(x, y + 1.0, z),
                Vec3::new(x, y + 1.0, z + 1.0),
                Vec3::new(x + 1.0, y + 1.0, z + 1.0),
                Vec3::new(x + 1.0, y + 1.0, z),
            ],
            Self::NegativeY => [
                Vec3::new(x, y, z),
                Vec3::new(x + 1.0, y, z),
                Vec3::new(x + 1.0, y, z + 1.0),
                Vec3::new(x, y, z + 1.0),
            ],
            Self::PositiveZ => [
                Vec3::new(x, y, z + 1.0),
                Vec3::new(x + 1.0, y, z + 1.0),
                Vec3::new(x + 1.0, y + 1.0, z + 1.0),
                Vec3::new(x, y + 1.0, z + 1.0),
            ],
            Self::NegativeZ => [
                Vec3::new(x, y, z),
                Vec3::new(x, y + 1.0, z),
                Vec3::new(x + 1.0, y + 1.0, z),
                Vec3::new(x + 1.0, y, z),
            ],
        }
    }
}
