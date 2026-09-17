//! Device-local packed-SVO storage with stable suballocations.

use std::ops::Range;

use bevy::render::{
    render_resource::{Buffer, BufferDescriptor, BufferUsages},
    renderer::{RenderDevice, RenderQueue},
};

const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub(super) struct SvoAllocation {
    pub(super) segment: usize,
    pub(super) offset: u64,
    pub(super) size: u64,
}

struct Segment {
    buffer: Buffer,
    free: Vec<Range<u64>>,
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
            if let Some(index) = entry
                .free
                .iter()
                .position(|range| range.end - range.start >= size)
            {
                let offset = entry.free[index].start;
                entry.free[index].start += size;
                if entry.free[index].is_empty() {
                    entry.free.swap_remove(index);
                }
                return Some(SvoAllocation {
                    segment,
                    offset,
                    size,
                });
            }
        }
        let remaining = self.budget_bytes.saturating_sub(self.reserved_bytes);
        if remaining < size || size > self.maximum_segment_bytes {
            return None;
        }
        let preferred = SEGMENT_BYTES
            .max(size.next_power_of_two())
            .min(remaining)
            .min(self.maximum_segment_bytes);
        let segment_size = preferred - preferred % self.alignment;
        if segment_size < size {
            return None;
        }
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("world packed-SVO arena segment"),
            size: segment_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let segment = self.segments.len();
        self.segments.push(Segment {
            buffer,
            free: vec![size..segment_size],
        });
        self.reserved_bytes += segment_size;
        Some(SvoAllocation {
            segment,
            offset: 0,
            size,
        })
    }

    pub(super) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub(super) fn segment_buffer(&self, segment: usize) -> &Buffer {
        &self.segments[segment].buffer
    }

    pub(super) fn free(&mut self, allocation: SvoAllocation) {
        let free = &mut self.segments[allocation.segment].free;
        free.push(allocation.offset..allocation.offset + allocation.size);
        free.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<Range<u64>> = Vec::with_capacity(free.len());
        for range in free.drain(..) {
            if let Some(previous) = merged.last_mut()
                && previous.end == range.start
            {
                previous.end = range.end;
            } else {
                merged.push(range);
            }
        }
        *free = merged;
    }
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
}
