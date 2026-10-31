//! Host-owned geometry buffers; page ownership and suballocation live on the GPU.

use bevy::render::{
    render_resource::{Buffer, BufferDescriptor, BufferInitDescriptor, BufferUsages, ShaderType},
    renderer::RenderDevice,
};

use super::{GENERATED_QUAD_SIZE, MAX_GPU_GEOMETRY_SEGMENTS, UVec4, ranges::segment_size};

// Approved page ABI: 128 B through 512 KiB, in powers of two. A slab is one
// largest page, and can be assigned any page class while it has no live pages.
pub(super) const MIN_PAGE_BYTES: u64 = 128;
pub(super) const SLAB_BYTES: u64 = 512 * 1024;
const BITMAP_WORDS: usize = (SLAB_BYTES / MIN_PAGE_BYTES / 32) as usize;

#[derive(Clone, Copy, ShaderType)]
pub(super) struct GpuPageSlab {
    location: UVec4,
    ownership: UVec4,
    bitmap: [UVec4; BITMAP_WORDS / 4],
}

/// Minimum binding size for the WGSL fault header followed by runtime slabs.
#[derive(Clone, Copy, ShaderType)]
pub(super) struct GpuPagePoolBinding {
    fault: UVec4,
    activity: UVec4,
    first_slab: GpuPageSlab,
}

/// All configured geometry capacity is reserved up front. GPU kernels assign
/// slab classes, claim page bits and reclaim pages; the CPU never reads model
/// sizes to decide allocations. No buffer is associated with a single class.
pub(super) struct GeometryArena {
    segments: Vec<Buffer>,
    pub(super) pages: Buffer,
    pub(super) slab_count: u32,
    reserved_bytes: u64,
}

impl GeometryArena {
    pub(super) fn new(device: &RenderDevice, budget_bytes: u64) -> Self {
        let limits = device.limits();
        let maximum_segment_bytes = limits
            .max_buffer_size
            .min(u64::from(limits.max_storage_buffer_binding_size))
            .min(u64::from(u32::MAX / 6) * GENERATED_QUAD_SIZE);
        let mut segments = Vec::new();
        let mut metadata = vec![0_u8; 32]; // Error status and aggregate diagnostics only.
        let mut reserved_bytes = 0;
        let mut slab_count = 0_u32;
        // A sub-slab budget tail cannot hold the largest approved page and is
        // left unreserved. It is never silently rounded up past the budget.
        while budget_bytes.saturating_sub(reserved_bytes) >= SLAB_BYTES {
            assert!(
                segments.len() < MAX_GPU_GEOMETRY_SEGMENTS as usize,
                "Out of Memory: geometry budget exceeds segment capacity"
            );
            let size = segment_size(
                SLAB_BYTES,
                SLAB_BYTES,
                budget_bytes,
                reserved_bytes,
                maximum_segment_bytes,
            )
            .expect("Out of Memory: device cannot bind a geometry slab");
            let segment = segments.len() as u32;
            for offset in (0..size).step_by(SLAB_BYTES as usize) {
                for value in [(offset / GENERATED_QUAD_SIZE) as u32, segment, 0, 0] {
                    metadata.extend_from_slice(&value.to_le_bytes());
                }
                metadata.resize(metadata.len() + 16 + BITMAP_WORDS * 4, 0);
                slab_count = slab_count
                    .checked_add(1)
                    .expect("geometry slab count exhausted");
            }
            segments.push(device.create_buffer(&BufferDescriptor {
                label: Some("world paged geometry segment"),
                size,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
            reserved_bytes += size;
        }
        assert!(
            slab_count != 0,
            "Out of Memory: geometry budget cannot cover one slab"
        );
        assert!(
            metadata.len() as u64 <= u64::from(limits.max_storage_buffer_binding_size),
            "Out of Memory: geometry page metadata exceeds the device binding limit"
        );
        let pages = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("world GPU page pool and error status"),
            contents: &metadata,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        });
        Self {
            segments,
            pages,
            slab_count,
            reserved_bytes,
        }
    }

    pub(super) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub(super) fn segment_buffer(&self, segment: usize) -> &Buffer {
        &self.segments[segment]
    }

    pub(super) fn reserved_bytes(&self) -> u64 {
        self.reserved_bytes
    }
}
