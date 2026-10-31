//! Background encoding and decoding for derived sparse voxel octrees.

use crate::{AtomicVoxel, CHUNK_EDGE_LENGTH, EMPTY_VOXEL_ID};
use crate::{ChunkVersion, VoxelChunkSvo};
use crossbeam::channel::{Receiver, Sender, select_biased, unbounded};
use roundo_contracts::{ConnectionId, SerializedPayload};
use roundo_dsa::data_structure::tree::{BreadthFirstLosslessSvo, Node, UnoptimizedOctree};
use std::sync::Arc;

/// Either a reusable SVO or immutable primitive voxel snapshot.
pub(crate) enum SvoSource {
    Cached(Arc<VoxelChunkSvo>),
    Primitive(Arc<[AtomicVoxel]>),
}

/// CPU-heavy conversion work transferred off the ECS thread.
pub(crate) enum DerivedSvoJob {
    Encode {
        connection_id: ConnectionId,
        chunk: ChunkVersion,
        source: SvoSource,
    },
    Decode {
        session_epoch: u64,
        chunk: ChunkVersion,
        payload: SerializedPayload,
    },
}

/// Successful conversion or contextual failure returned to the owner.
pub(crate) enum DerivedSvoResult {
    Encoded {
        connection_id: ConnectionId,
        chunk: ChunkVersion,
        payload: SerializedPayload,
    },
    Decoded {
        session_epoch: u64,
        chunk: ChunkVersion,
        svo: VoxelChunkSvo,
    },
    Failed {
        connection_id: Option<ConnectionId>,
        session_epoch: Option<u64>,
        chunk: ChunkVersion,
        error: String,
    },
}

#[derive(Clone)]
/// Non-blocking endpoint for a dedicated SVO conversion thread.
/// The server can prioritize edited versions; clients instead admit bounded
/// spatially selected batches to the regular lane. No lane preempts a conversion
/// already executing. Callers still validate exact versions.
pub(crate) struct DerivedSvoWorker {
    jobs: Sender<DerivedSvoJob>,
    priority_jobs: Sender<DerivedSvoJob>,
    results: Receiver<DerivedSvoResult>,
    priority_results: Receiver<DerivedSvoResult>,
}

impl DerivedSvoWorker {
    /// Starts a named worker thread and returns its ECS-side endpoint.
    pub(crate) fn spawn(name: &'static str) -> Self {
        let (jobs, job_receiver) = unbounded();
        let (priority_jobs, priority_job_receiver) = unbounded();
        let (result_sender, results) = unbounded();
        let (priority_result_sender, priority_results) = unbounded();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                run_worker(
                    job_receiver,
                    priority_job_receiver,
                    result_sender,
                    priority_result_sender,
                )
            })
            .expect("derived SVO worker thread must start");
        Self {
            jobs,
            priority_jobs,
            results,
            priority_results,
        }
    }

    /// Submits work or returns it intact when the worker has stopped.
    pub(crate) fn submit(&self, job: DerivedSvoJob) -> Result<(), DerivedSvoJob> {
        self.jobs.send(job).map_err(|error| error.0)
    }

    /// Submits an edited version ahead of initial terrain awaiting conversion.
    pub(crate) fn submit_priority(&self, job: DerivedSvoJob) -> Result<(), DerivedSvoJob> {
        self.priority_jobs.send(job).map_err(|error| error.0)
    }

    /// Receives priority-lane results before regular-lane results.
    pub(crate) fn try_receive(&self) -> Option<DerivedSvoResult> {
        self.priority_results
            .try_recv()
            .ok()
            .or_else(|| self.results.try_recv().ok())
    }
}

// Worker lifetime is bounded by endpoint availability.
fn run_worker(
    jobs: Receiver<DerivedSvoJob>,
    priority_jobs: Receiver<DerivedSvoJob>,
    results: Sender<DerivedSvoResult>,
    priority_results: Sender<DerivedSvoResult>,
) {
    loop {
        // Both input senders belong to each worker handle, so disconnection of
        // either input means all owners have dropped; there is no orphan lane.
        let (job, priority) = select_biased! {
            recv(priority_jobs) -> job => match job {
                Ok(job) => (job, true),
                Err(_) => break,
            },
            recv(jobs) -> job => match job {
                Ok(job) => (job, false),
                Err(_) => break,
            },
        };
        let result = match job {
            DerivedSvoJob::Encode {
                connection_id,
                chunk,
                source,
            } => encode_chunk(connection_id, chunk, source),
            DerivedSvoJob::Decode {
                session_epoch,
                chunk,
                payload,
            } => decode_chunk(session_epoch, chunk, payload),
        };
        let output = if priority {
            &priority_results
        } else {
            &results
        };
        if output.send(result).is_err() {
            break;
        }
        std::thread::yield_now();
    }
}

// Reuses cached SVOs and materializes primitive snapshots only when required.
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
                    session_epoch: None,
                    chunk,
                    error: error.to_string(),
                };
            }
        },
    };
    let encoded = SerializedPayload::encode(&svo);
    match encoded {
        Ok(payload) => DerivedSvoResult::Encoded {
            connection_id,
            chunk,
            payload,
        },
        Err(error) => DerivedSvoResult::Failed {
            connection_id: Some(connection_id),
            session_epoch: None,
            chunk,
            error: error.to_string(),
        },
    }
}

// Builds the editable tree before breadth-first lossless compaction.
fn svo_from_primitive_voxels(
    voxels: &[AtomicVoxel],
) -> Result<VoxelChunkSvo, roundo_dsa::data_structure::tree::OptimizedOctreeError> {
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

// Descends by high-to-low coordinate bits to select each octant.
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

// Preserves chunk identity across payload decoding failures.
fn decode_chunk(
    session_epoch: u64,
    chunk: ChunkVersion,
    payload: SerializedPayload,
) -> DerivedSvoResult {
    let decoded = payload.decode();
    match decoded {
        Ok(svo) => DerivedSvoResult::Decoded {
            session_epoch,
            chunk,
            svo,
        },
        Err(error) => DerivedSvoResult::Failed {
            connection_id: None,
            session_epoch: Some(session_epoch),
            chunk,
            error: error.to_string(),
        },
    }
}
