//! Dedicated worker for deterministic procedural chunk generation.

use super::WorldChunkGenerator;
use crate::{ChunkId, GeneratedChunk};
use roundo_infrastructure::toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};

/// Immutable generation request transferred off the ECS thread.
pub(super) struct ChunkGenerationJob {
    pub id: ChunkId,
    pub generator: WorldChunkGenerator,
    pub installation: Option<crate::CoordinateIncarnation>,
}

/// Generated chunk paired with its owning local coordinate.
pub(super) struct ChunkGenerationResult {
    pub id: ChunkId,
    pub chunk: GeneratedChunk,
    pub installation: Option<crate::CoordinateIncarnation>,
}

#[derive(Clone)]
/// Non-blocking ECS-side endpoint for the generation thread.
pub(super) struct ChunkGenerationWorker {
    pipe: CrossbeamThreadPipeEndpointA<ChunkGenerationJob, ChunkGenerationResult>,
}

impl ChunkGenerationWorker {
    pub fn spawn() -> Self {
        let pipe = CrossbeamThreadPipe::new();
        let worker_pipe = pipe.endpoint_b();
        std::thread::Builder::new()
            .name("roundo-world-pcg".to_string())
            .spawn(move || run_worker(worker_pipe))
            .expect("world PCG worker thread must start");
        Self {
            pipe: pipe.endpoint_a(),
        }
    }

    pub fn submit(&self, job: ChunkGenerationJob) -> Result<(), ChunkGenerationJob> {
        self.pipe.try_send(job)
    }

    pub fn try_receive(&self) -> Option<ChunkGenerationResult> {
        self.pipe.try_receive()
    }
}

// Channel closure terminates the worker when its World is dropped.
fn run_worker(pipe: CrossbeamThreadPipeEndpointB<ChunkGenerationJob, ChunkGenerationResult>) {
    while let Some(job) = pipe.receive() {
        let chunk = job.generator.generate_chunk(job.id.coordinate);
        if pipe
            .try_send(ChunkGenerationResult {
                id: job.id,
                chunk,
                installation: job.installation,
            })
            .is_err()
        {
            break;
        }
        std::thread::yield_now();
    }
}
