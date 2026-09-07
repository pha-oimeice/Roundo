use crate::{ChunkCoordinate, GeneratedChunk, LocalCoordinateId, SuperflatGenerator};
use roundo_toolbox::{
    CrossbeamThreadPipe, CrossbeamThreadPipeEndpointA, CrossbeamThreadPipeEndpointB,
};

pub(super) struct ChunkGenerationJob {
    pub local_coordinate_id: LocalCoordinateId,
    pub coordinate: ChunkCoordinate,
    pub generator: SuperflatGenerator,
}

pub(super) struct ChunkGenerationResult {
    pub local_coordinate_id: LocalCoordinateId,
    pub chunk: GeneratedChunk,
}

#[derive(Clone)]
pub(super) struct ChunkGenerationWorker {
    pipe: CrossbeamThreadPipeEndpointA<ChunkGenerationJob, ChunkGenerationResult>,
}

impl ChunkGenerationWorker {
    pub fn spawn() -> Self {
        let pipe = CrossbeamThreadPipe::new();
        let worker_pipe = pipe.endpoint_b();
        std::thread::Builder::new()
            .name("roundo-world-streaming-pcg".to_string())
            .spawn(move || run_worker(worker_pipe))
            .expect("world streaming PCG worker thread must start");
        Self {
            pipe: pipe.endpoint_a(),
        }
    }

    pub fn submit(&self, job: ChunkGenerationJob) {
        let _ = self.pipe.try_send(job);
    }

    pub fn try_receive(&self) -> Option<ChunkGenerationResult> {
        self.pipe.try_receive()
    }
}

fn run_worker(pipe: CrossbeamThreadPipeEndpointB<ChunkGenerationJob, ChunkGenerationResult>) {
    while let Some(job) = pipe.receive() {
        let chunk =
            job.generator
                .generate_chunk(job.coordinate[0], job.coordinate[1], job.coordinate[2]);
        if pipe
            .try_send(ChunkGenerationResult {
                local_coordinate_id: job.local_coordinate_id,
                chunk,
            })
            .is_err()
        {
            break;
        }
        std::thread::yield_now();
    }
}
