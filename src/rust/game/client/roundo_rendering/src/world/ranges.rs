//! Pure first-fit ranges and checked sizing shared by the GPU arena adapters.

use std::ops::Range;

const DEFAULT_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

/// Free ranges belonging to exactly one segment, in first-fit selection order.
///
/// Exhaustion uses swap_remove, so allocation need not leave ranges sorted.
/// Only adjacent ranges are coalesced on free; double-free violates the caller's
/// ownership contract and is not detected here.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FreeRanges(Vec<Range<u64>>);

impl FreeRanges {
    pub(super) fn new(range: Range<u64>) -> Self {
        Self(vec![range])
    }

    pub(super) fn allocate(&mut self, size: u64) -> Option<u64> {
        let index = self
            .0
            .iter()
            .position(|range| range.end - range.start >= size)?;
        let offset = self.0[index].start;
        let end = offset.checked_add(size)?;
        self.0[index].start = end;
        if self.0[index].is_empty() {
            self.0.swap_remove(index);
        }
        Some(offset)
    }

    pub(super) fn free(&mut self, range: Range<u64>) {
        self.0.push(range);
        self.0.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<Range<u64>> = Vec::with_capacity(self.0.len());
        for range in self.0.drain(..) {
            if let Some(previous) = merged.last_mut()
                && previous.end == range.start
            {
                previous.end = range.end;
            } else {
                merged.push(range);
            }
        }
        self.0 = merged;
    }
}

pub(super) fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment.checked_sub(1)?)?
        .checked_div(alignment)?
        .checked_mul(alignment)
}

/// Plan growth without changing any ranges or the adapter's reservation ledger.
/// Prefer 64 MiB, clip to device/budget limits, then round down to alignment.
pub(super) fn segment_size(
    size: u64,
    alignment: u64,
    budget_bytes: u64,
    reserved_bytes: u64,
    maximum_segment_bytes: u64,
) -> Option<u64> {
    let remaining = budget_bytes.checked_sub(reserved_bytes)?;
    if remaining < size || size > maximum_segment_bytes {
        return None;
    }
    let preferred = DEFAULT_SEGMENT_BYTES
        .max(size.checked_next_power_of_two()?)
        .min(maximum_segment_bytes)
        .min(remaining);
    let segment_size = preferred.checked_sub(preferred.checked_rem(alignment)?)?;
    (segment_size >= size).then_some(segment_size)
}
