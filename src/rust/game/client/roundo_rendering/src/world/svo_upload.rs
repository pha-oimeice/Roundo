//! Event-driven SVO packing, submission, completion and retirement.
//!
//! Only this worker mutates the SVO arena. Render schedules exchange immutable
//! inputs and completed allocations with it; no frame epoch drives its loop.

use super::svo_arena::{SvoAllocation, SvoArena};
use super::{ChunkId, MAX_GPU_CHUNK_DESCRIPTORS, VoxelChunkSvo};
use bevy::render::{
    render_resource::{Buffer, PollType},
    renderer::{RenderDevice, RenderQueue},
};
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
};

struct Upload {
    request: u64,
    svo: Arc<VoxelChunkSvo>,
}

pub(super) struct ReadySvo {
    pub(super) id: ChunkId,
    pub(super) request: u64,
    pub(super) allocation: SvoAllocation,
}

#[derive(Default)]
struct Mailbox {
    next_request: u64,
    // One latest input/result per live identity, bounded by the descriptor ABI.
    // A replacement never creates an unbounded FIFO of obsolete revisions.
    latest: HashMap<ChunkId, u64>,
    pending: HashMap<ChunkId, Upload>,
    ready: HashMap<ChunkId, ReadySvo>,
    retired: Vec<SvoAllocation>,
    buffers: Vec<Buffer>,
    stopping: bool,
    failure: Option<String>,
}

#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
}

pub(super) struct SvoUploads {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl SvoUploads {
    pub(super) fn spawn(device: RenderDevice, queue: RenderQueue, budget_bytes: u64) -> Self {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("roundo-svo-upload".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    run(&worker_shared, &device, &queue, budget_bytes)
                }));
                let failure = match outcome {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => error,
                    Err(error) => error
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| error.downcast_ref::<&str>().map(|text| text.to_string()))
                        .unwrap_or_else(|| "SVO upload worker panicked".into()),
                };
                bevy::log::error!("SVO upload worker stopped: {failure}");
                let mut mailbox = worker_shared
                    .mailbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                mailbox.failure = Some(failure);
                worker_shared.wake.notify_all();
            })
            .expect("SVO upload worker must start");
        Self {
            shared,
            worker: Some(worker),
        }
    }

    fn mailbox(&self) -> MutexGuard<'_, Mailbox> {
        let mailbox = self.shared.mailbox.lock().expect("SVO mailbox lock");
        assert!(
            mailbox.failure.is_none(),
            "SVO worker failed: {:?}",
            mailbox.failure
        );
        mailbox
    }

    /// Coalesces the latest immutable input without packing or waiting for GPU work.
    pub(super) fn request(&self, id: ChunkId, svo: Arc<VoxelChunkSvo>) -> u64 {
        let mut mailbox = self.mailbox();
        assert!(
            mailbox.latest.contains_key(&id)
                || mailbox.latest.len() < MAX_GPU_CHUNK_DESCRIPTORS as usize,
            "SVO upload identity capacity exhausted"
        );
        mailbox.next_request = mailbox
            .next_request
            .checked_add(1)
            .expect("SVO request exhausted");
        let request = mailbox.next_request;
        mailbox.latest.insert(id, request);
        mailbox.pending.insert(id, Upload { request, svo });
        if let Some(ready) = mailbox.ready.remove(&id) {
            mailbox.retired.push(ready.allocation);
        }
        self.shared.wake.notify_one();
        request
    }

    pub(super) fn cancel(&self, id: ChunkId) {
        let mut mailbox = self.mailbox();
        mailbox.latest.remove(&id);
        mailbox.pending.remove(&id);
        if let Some(ready) = mailbox.ready.remove(&id) {
            mailbox.retired.push(ready.allocation);
        }
        self.shared.wake.notify_one();
    }

    /// The caller must have submitted the last commands that can read this range
    /// and must not encode any more uses. The worker fences the shared queue before
    /// reuse, independently of later render frames or frame-completion callbacks.
    pub(super) fn retire(&self, allocation: SvoAllocation) {
        self.mailbox().retired.push(allocation);
        self.shared.wake.notify_one();
    }

    /// Returns only submission-complete allocations, plus read-only buffer handles.
    /// Consuming results on a frame does not drive the worker's progress.
    pub(super) fn take_ready(&self) -> (Vec<Buffer>, Vec<ReadySvo>) {
        let mut mailbox = self.mailbox();
        let buffers = mailbox.buffers.clone();
        let ready = mailbox.ready.drain().map(|(_, ready)| ready).collect();
        (buffers, ready)
    }
}

impl Drop for SvoUploads {
    fn drop(&mut self) {
        {
            let mut mailbox = self
                .shared
                .mailbox
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            mailbox.stopping = true;
            self.shared.wake.notify_one();
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            bevy::log::error!("failed to join SVO upload worker");
        }
    }
}

fn complete_submission(device: &RenderDevice, queue: &RenderQueue) -> Result<(), String> {
    // Explicit submission also flushes pending write_buffer transfers. A scoped
    // wait progresses mapping/completion callbacks even when no frame is rendered.
    let submission = queue.submit([]);
    device
        .poll(PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|error| format!("SVO submission completion failed: {error}"))?;
    Ok(())
}

fn run(
    shared: &Shared,
    device: &RenderDevice,
    queue: &RenderQueue,
    budget_bytes: u64,
) -> Result<(), String> {
    let mut arena = SvoArena::new(device, budget_bytes);
    loop {
        let (pending, retired) = {
            let mut mailbox = shared.mailbox.lock().expect("SVO mailbox lock");
            while !mailbox.stopping && mailbox.pending.is_empty() && mailbox.retired.is_empty() {
                mailbox = shared.wake.wait(mailbox).expect("SVO mailbox wait");
            }
            if mailbox.stopping {
                return Ok(());
            }
            (
                std::mem::take(&mut mailbox.pending),
                std::mem::take(&mut mailbox.retired),
            )
        };
        if !retired.is_empty() {
            complete_submission(device, queue)?;
            for allocation in retired {
                arena.free(allocation);
            }
        }

        let mut uploaded = Vec::new();
        for (id, upload) in pending {
            {
                let mailbox = shared.mailbox.lock().expect("SVO mailbox lock");
                if mailbox.stopping {
                    return Ok(());
                }
                if mailbox.latest.get(&id) != Some(&upload.request) {
                    continue;
                }
            }
            let packed = upload.svo.pack_with(|voxel| voxel.0);
            let allocation = arena
                .upload(device, queue, &packed.node_bytes())
                .ok_or_else(|| "packed SVO arena budget cannot cover the loaded set".to_string())?;
            uploaded.push(ReadySvo {
                id,
                request: upload.request,
                allocation,
            });
        }
        if uploaded.is_empty() {
            continue;
        }
        complete_submission(device, queue)?;
        let mut mailbox = shared.mailbox.lock().expect("SVO mailbox lock");
        mailbox.buffers = arena.buffers();
        for ready in uploaded {
            if mailbox.latest.get(&ready.id) != Some(&ready.request) {
                arena.free(ready.allocation);
            } else if let Some(previous) = mailbox.ready.insert(ready.id, ready) {
                // This result has never left the mailbox; its upload is complete
                // and no render submission can refer to it.
                arena.free(previous.allocation);
            }
        }
    }
}
