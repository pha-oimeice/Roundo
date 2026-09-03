use crate::{AtomicVoxel, CHUNK_EDGE_LENGTH, EMPTY_VOXEL_ID};
use crate::{ChunkVersion, VoxelChunkSvo};
use roundo_algorithm::tree::{BreadthFirstLosslessSvo, Node, UnoptimizedOctree};
use roundo_networking::{ConnectionId, SerializedPayload};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};
use std::sync::Arc;

pub(crate) enum SvoSource {
    Cached(Arc<VoxelChunkSvo>),
    Primitive(Arc<[AtomicVoxel]>),
}

pub(crate) enum DerivedSvoJob {
    Encode {
        connection_id: ConnectionId,
        chunk: ChunkVersion,
        source: SvoSource,
    },
    Decode {
        chunk: ChunkVersion,
        payload: SerializedPayload,
    },
}

pub(crate) enum DerivedSvoResult {
    Encoded {
        connection_id: ConnectionId,
        chunk: ChunkVersion,
        payload: SerializedPayload,
    },
    Decoded {
        chunk: ChunkVersion,
        svo: VoxelChunkSvo,
    },
    Failed {
        connection_id: Option<ConnectionId>,
        chunk: ChunkVersion,
        error: String,
    },
}

#[derive(Clone)]
pub(crate) struct DerivedSvoWorker {
    pipe: CrossbeamThreadPipeEndpointA<DerivedSvoJob, DerivedSvoResult>,
}

impl DerivedSvoWorker {
    pub(crate) fn spawn(name: &'static str) -> Self {
        let pipe = CrossbeamThreadPipe::new();
        let worker_pipe = pipe.endpoint_b();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || run_worker(worker_pipe))
            .expect("derived SVO worker thread must start");
        Self {
            pipe: pipe.endpoint_a(),
        }
    }

    pub(crate) fn submit(&self, job: DerivedSvoJob) {
        let _ = self.pipe.try_send(job);
    }

    pub(crate) fn try_receive(&self) -> Option<DerivedSvoResult> {
        self.pipe.try_receive()
    }
}

fn run_worker(pipe: CrossbeamThreadPipeEndpointB<DerivedSvoJob, DerivedSvoResult>) {
    while let Some(job) = pipe.receive() {
        let result = match job {
            DerivedSvoJob::Encode {
                connection_id,
                chunk,
                source,
            } => encode_chunk(connection_id, chunk, source),
            DerivedSvoJob::Decode { chunk, payload } => decode_chunk(chunk, payload),
        };
        if pipe.try_send(result).is_err() {
            break;
        }
        std::thread::yield_now();
    }
}

fn encode_chunk(
    connection_id: ConnectionId,
    chunk: ChunkVersion,
    source: SvoSource,
) -> DerivedSvoResult {
    let svo = match source {
        SvoSource::Cached(svo) => (*svo).clone(),
        SvoSource::Primitive(source) => match svo_from_primitive_voxels(&source) {
            Ok(svo) => svo,
            Err(error) => {
                return DerivedSvoResult::Failed {
                    connection_id: Some(connection_id),
                    chunk,
                    error: error.to_string(),
                };
            }
        },
    };
    match SerializedPayload::encode(&svo) {
        Ok(payload) => DerivedSvoResult::Encoded {
            connection_id,
            chunk,
            payload,
        },
        Err(error) => DerivedSvoResult::Failed {
            connection_id: Some(connection_id),
            chunk,
            error: error.to_string(),
        },
    }
}

fn svo_from_primitive_voxels(
    voxels: &[AtomicVoxel],
) -> Result<VoxelChunkSvo, roundo_algorithm::tree::OptimizedOctreeError> {
    let edge = CHUNK_EDGE_LENGTH as usize;
    let mut source = UnoptimizedOctree::new(0, EMPTY_VOXEL_ID);
    for (index, &voxel) in voxels.iter().enumerate() {
        if voxel == EMPTY_VOXEL_ID {
            continue;
        }
        let position = [index % edge, index / edge % edge, index / edge.pow(2)];
        insert_voxel(&mut source.root, position, voxel);
    }
    BreadthFirstLosslessSvo::from_unoptimized_mapped(
        &source,
        CHUNK_EDGE_LENGTH.ilog2() as u8,
        |data| *data,
    )
}

fn insert_voxel(root: &mut Node<AtomicVoxel, 8>, position: [usize; 3], voxel: AtomicVoxel) {
    let depth = (CHUNK_EDGE_LENGTH as usize).ilog2() as usize;
    let mut node = root;
    let mut node_id = 0_usize;
    for level in 0..depth {
        let bit = depth - level - 1;
        let octant = ((position[0] >> bit) & 1)
            | (((position[1] >> bit) & 1) << 1)
            | (((position[2] >> bit) & 1) << 2);
        node_id = node_id * 8 + octant + 1;
        node = node.children[octant]
            .get_or_insert_with(|| Box::new(Node::new(node_id as u32, EMPTY_VOXEL_ID)))
            .as_mut();
    }
    node.data = voxel;
}

fn decode_chunk(chunk: ChunkVersion, payload: SerializedPayload) -> DerivedSvoResult {
    match payload.decode() {
        Ok(svo) => DerivedSvoResult::Decoded { chunk, svo },
        Err(error) => DerivedSvoResult::Failed {
            connection_id: None,
            chunk,
            error: error.to_string(),
        },
    }
}
