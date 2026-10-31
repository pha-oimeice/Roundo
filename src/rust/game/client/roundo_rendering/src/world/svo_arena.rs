//! Device-local packed-SVO storage with stable suballocations.

use bevy::render::{
    render_resource::{Buffer, BufferDescriptor, BufferUsages},
    renderer::{RenderDevice, RenderQueue},
};

use super::ranges::{FreeRanges, align_up, segment_size};

#[derive(Clone, Copy, Debug)]
pub(super) struct SvoAllocation {
    pub(super) segment: usize,
    pub(super) offset: u64,
    pub(super) size: u64,
}

struct Segment {
    buffer: Buffer,
    free: FreeRanges,
}

/// Packed SVOs remain independent ranges while sharing a small number of GPU buffers.
pub(super) struct SvoArena {
    segments: Vec<Segment>,
    budget_bytes: u64,
    reserved_bytes: u64,
    maximum_segment_bytes: u64,
    alignment: u64,
}

impl SvoArena {
    pub(super) fn new(device: &RenderDevice, budget_bytes: u64) -> Self {
        Self {
            segments: Vec::new(),
            budget_bytes,
            reserved_bytes: 0,
            maximum_segment_bytes: device.limits().max_buffer_size,
            alignment: u64::from(device.limits().min_storage_buffer_offset_alignment).max(16),
        }
    }

    pub(super) fn upload(
        &mut self,
        device: &RenderDevice,
        queue: &RenderQueue,
        bytes: &[u8],
    ) -> Option<SvoAllocation> {
        let size = align_up(bytes.len() as u64, self.alignment)?;
        let allocation = self.allocate(device, size)?;
        queue.write_buffer(
            &self.segments[allocation.segment].buffer,
            allocation.offset,
            bytes,
        );
        Some(allocation)
    }

    fn allocate(&mut self, device: &RenderDevice, size: u64) -> Option<SvoAllocation> {
        for (segment, entry) in self.segments.iter_mut().enumerate() {
            if let Some(offset) = entry.free.allocate(size) {
                return Some(SvoAllocation {
                    segment,
                    offset,
                    size,
                });
            }
        }
        let segment_size = segment_size(
            size,
            self.alignment,
            self.budget_bytes,
            self.reserved_bytes,
            self.maximum_segment_bytes,
        )?;
        let reserved_bytes = self.reserved_bytes.checked_add(segment_size)?;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("world packed-SVO arena segment"),
            size: segment_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let segment = self.segments.len();
        self.segments.push(Segment {
            buffer,
            free: FreeRanges::new(size..segment_size),
        });
        self.reserved_bytes = reserved_bytes;
        Some(SvoAllocation {
            segment,
            offset: 0,
            size,
        })
    }

    pub(super) fn buffers(&self) -> Vec<Buffer> {
        self.segments
            .iter()
            .map(|segment| segment.buffer.clone())
            .collect()
    }

    pub(super) fn free(&mut self, allocation: SvoAllocation) {
        self.segments[allocation.segment]
            .free
            .free(allocation.offset..allocation.offset + allocation.size);
    }
}
