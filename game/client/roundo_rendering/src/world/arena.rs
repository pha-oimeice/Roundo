//! Shared GPU geometry storage with aligned suballocation and coalescing.

use std::ops::Range;

use bevy::render::{
    render_resource::{Buffer, BufferDescriptor, BufferUsages},
    renderer::RenderDevice,
};

use super::GENERATED_QUAD_SIZE;

// Segments grow independently while remaining under the global byte budget.
const DEFAULT_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

/// Location of one live geometry payload within an arena segment.
///
/// Copying this value does not create another allocation. The location becomes
/// invalid after the original allocation is freed and may then be reused.
#[derive(Clone, Copy, Debug)]
pub(super) struct GeometryAllocation {
    pub(super) segment: usize,
    pub(super) offset: u64,
    pub(super) size: u64,
}

/// GPU buffer paired with sorted, non-overlapping free ranges.
struct GeometryArenaSegment {
    buffer: Buffer,
    free: Vec<Range<u64>>,
}

/// Budgeted allocator spanning retained GPU storage-buffer segments.
///
/// Freeing payloads makes their ranges reusable but does not destroy segments
/// or reduce reserved bytes. The arena is not a moving allocator: live offsets
/// remain stable until explicitly freed.
pub(super) struct GeometryArena {
    segments: Vec<GeometryArenaSegment>,
    budget_bytes: u64,
    reserved_bytes: u64,
    maximum_segment_bytes: u64,
    alignment: u64,
}

impl GeometryArena {
    /// Creates an empty arena using the device's storage-offset alignment.
    ///
    /// `budget_bytes` limits total segment reservation, not currently live
    /// payload bytes.
    pub(super) fn new(render_device: &RenderDevice, budget_bytes: u64) -> Self {
        let limits = render_device.limits();
        Self {
            segments: Vec::new(),
            budget_bytes,
            reserved_bytes: 0,
            maximum_segment_bytes: limits.max_buffer_size,
            alignment: u64::from(limits.min_storage_buffer_offset_alignment).max(1),
        }
    }

    /// Allocates a stable, aligned range using deterministic first fit.
    ///
    /// Requests smaller than one generated vertex reserve one vertex. Returns
    /// `None` on arithmetic overflow or when device/budget limits cannot fit a
    /// new segment; failure leaves existing allocations unchanged.
    pub(super) fn allocate(
        &mut self,
        render_device: &RenderDevice,
        requested_bytes: u64,
    ) -> Option<GeometryAllocation> {
        let size = align_up(requested_bytes.max(GENERATED_QUAD_SIZE), self.alignment)?;
        for (segment_index, segment) in self.segments.iter_mut().enumerate() {
            if let Some(allocation) = allocate_from_ranges(&mut segment.free, segment_index, size) {
                return Some(allocation);
            }
        }

        // New segments prefer 64 MiB but shrink to device and budget constraints.
        let remaining = self.budget_bytes.saturating_sub(self.reserved_bytes);
        if remaining < size || size > self.maximum_segment_bytes {
            return None;
        }
        let preferred = DEFAULT_SEGMENT_BYTES
            .max(size.next_power_of_two())
            .min(self.maximum_segment_bytes)
            .min(remaining);
        let segment_size = preferred - preferred % self.alignment;
        if segment_size < size {
            return None;
        }
        let buffer = render_device.create_buffer(&BufferDescriptor {
            label: Some("world shared geometry arena segment"),
            size: segment_size,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let segment_index = self.segments.len();
        self.segments.push(GeometryArenaSegment {
            buffer,
            free: vec![0..segment_size],
        });
        self.reserved_bytes += segment_size;
        allocate_from_ranges(&mut self.segments[segment_index].free, segment_index, size)
    }

    /// Returns the GPU buffer containing a live allocation.
    ///
    /// # Panics
    ///
    /// Panics if `allocation` did not originate from this arena. A freed
    /// allocation may still address its retained segment but no longer owns its
    /// range and must not be used for rendering.
    /// Number of physical device-local segments currently reserved.
    pub(super) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Returns one segment buffer for batched rendering.
    pub(super) fn segment_buffer(&self, segment: usize) -> &Buffer {
        &self.segments[segment].buffer
    }

    /// Frees one live allocation and coalesces adjacent free ranges.
    ///
    /// The allocation must originate from this arena and be freed exactly once.
    /// Segments remain allocated for reuse, so this does not lower the arena's
    /// reserved-byte budget.
    pub(super) fn free(&mut self, allocation: GeometryAllocation) {
        let ranges = &mut self.segments[allocation.segment].free;
        ranges.push(allocation.offset..allocation.offset + allocation.size);
        ranges.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
        for range in ranges.drain(..) {
            if let Some(previous) = merged.last_mut()
                && previous.end == range.start
            {
                previous.end = range.end;
            } else {
                merged.push(range);
            }
        }
        *ranges = merged;
    }
}

// Checked arithmetic rejects sizes that overflow during alignment.
fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
}

// First-fit allocation keeps selection deterministic and inexpensive.
fn allocate_from_ranges(
    ranges: &mut Vec<Range<u64>>,
    segment: usize,
    size: u64,
) -> Option<GeometryAllocation> {
    let index = ranges
        .iter()
        .position(|range| range.end - range.start >= size)?;
    let offset = ranges[index].start;
    ranges[index].start += size;
    if ranges[index].is_empty() {
        ranges.swap_remove(index);
    }
    Some(GeometryAllocation {
        segment,
        offset,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_ranges_coalesce_after_retirement() {
        let mut ranges = vec![0..1024];
        let first = allocate_from_ranges(&mut ranges, 0, 256).unwrap();
        let second = allocate_from_ranges(&mut ranges, 0, 256).unwrap();
        assert_eq!(ranges, vec![512..1024]);

        ranges.push(first.offset..first.offset + first.size);
        ranges.push(second.offset..second.offset + second.size);
        ranges.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<Range<u64>> = Vec::new();
        for range in ranges {
            if let Some(previous) = merged.last_mut()
                && previous.end == range.start
            {
                previous.end = range.end;
            } else {
                merged.push(range);
            }
        }
        assert_eq!(merged, vec![0..1024]);
    }
}
