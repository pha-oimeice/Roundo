//! Asynchronous error reporting and aggregate diagnostics only. Per-model
//! counts, LOD and page allocation never wait for or consume this readback.

use super::*;
use std::sync::{Arc, Mutex};

type StatusResult = Arc<Mutex<Option<Result<Vec<u8>, String>>>>;
pub(super) struct PendingGpuStatus {
    result: StatusResult,
}

pub(super) fn collect_gpu_status(residency: &mut WorldResidency) {
    let Some(pending) = &residency.pending_status else {
        return;
    };
    let result = pending.result.lock().expect("GPU status lock").take();
    let Some(result) = result else {
        return;
    };
    residency.pending_status = None;
    let bytes = result.unwrap_or_else(|error| panic!("world GPU status readback failed: {error}"));
    assert_eq!(bytes.len(), 32, "invalid world GPU status size");
    let words: [u32; 8] = std::array::from_fn(|index| {
        u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
    });
    if words[0] & 1 != 0 {
        panic!(
            "Out of Memory: GPU geometry page pool exhausted; slot={}, requested_bytes={}, page_class={}, fault_bits={}",
            words[1], words[2], words[3], words[0]
        );
    }
    assert_eq!(
        words[0], 0,
        "GPU geometry generation failed: status={words:?}"
    );
    residency.gpu_activity.copy_from_slice(&words[4..8]);
}

pub(super) fn read_gpu_status(
    context: &mut RenderContext,
    device: &RenderDevice,
    pages: &Buffer,
) -> PendingGpuStatus {
    let staging = device.create_buffer(&bevy::render::render_resource::BufferDescriptor {
        label: Some("world GPU error status readback"),
        size: 32,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    context
        .command_encoder()
        .copy_buffer_to_buffer(pages, 0, &staging, 0, 32);
    let result: StatusResult = Arc::new(Mutex::new(None));
    let completed = Arc::clone(&result);
    let mapped = staging.clone();
    context.command_encoder().map_buffer_on_submit(
        &staging,
        bevy::render::render_resource::MapMode::Read,
        ..,
        move |status| {
            let bytes = status
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    let bytes = mapped
                        .slice(..)
                        .get_mapped_range()
                        .map(|view| view.to_vec())
                        .map_err(|error| error.to_string());
                    mapped.unmap();
                    bytes
                });
            *completed.lock().expect("GPU status lock") = Some(bytes);
        },
    );
    PendingGpuStatus { result }
}
