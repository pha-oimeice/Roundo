// View-dependent model choice and fixed-class GPU paging. No model sizes are
// returned to the host. Class 0 is 128 B, class 12 is 512 KiB; a slab is 512 KiB.
struct ChunkDescriptor {
    world_from_chunk: mat4x4<f32>, chunk_coordinate: vec4<i32>,
    draws: array<vec4<u32>, 5>, metadata: vec4<u32>, svo: vec4<u32>, neighbor_slots: array<vec4<u32>, 2>,
}
struct Measurement { count: atomic<u32>, error_bits: atomic<u32>, reserved0: u32, reserved1: u32 }
struct ModelState { generations: vec4<u32>, allocation: vec4<u32> }
struct Request { destination: vec4<u32>, bounds: vec4<u32> }
// Kept byte-identical to arena.rs / world_greedy_emit.wesl. location is a
// geometry record offset and segment; class is index+1, zero means unassigned.
struct PageSlab {
    location: vec4<u32>, page_class: atomic<u32>, live_pages: atomic<u32>, reserved0: u32, reserved1: u32,
    bitmap: array<atomic<u32>, 128>,
}
struct PagePool { fault: array<atomic<u32>, 4>, activity: array<atomic<u32>, 4>, slabs: array<PageSlab> }
struct ModelView { clip_from_world: mat4x4<f32>, viewport: vec4<u32> }
struct SelectPass { descriptor_count: u32, slab_count: u32, view_count: u32, tolerance: f32 }
@group(0) @binding(0) var<storage, read_write> descriptors: array<ChunkDescriptor>;
@group(0) @binding(1) var<storage, read_write> measurements: array<Measurement>;
@group(0) @binding(2) var<storage, read_write> models: array<ModelState>;
@group(0) @binding(3) var<storage, read_write> requests: array<Request>;
@group(0) @binding(4) var<storage, read_write> pool: PagePool;
@group(0) @binding(5) var<storage, read> views: array<ModelView>;
@group(0) @binding(6) var<uniform> selection: SelectPass;
@group(0) @binding(7) var<storage, read_write> dispatch: array<atomic<u32>>;
const INVALID: u32 = 0xffffffffu;
const MIN_PAGE_RECORDS: u32 = 8u;
const SLAB_RECORDS: u32 = 32768u;

fn release_page(slab: u32, page: u32) {
    let flag = 1u << (page % 32u);
    let previous = atomicAnd(&pool.slabs[slab].bitmap[page / 32u], ~flag);
    if ((previous & flag) == 0u) { atomicOr(&pool.fault[0], 8u); return; }
    if (atomicSub(&pool.slabs[slab].live_pages, 1u) == 0u) { atomicOr(&pool.fault[0], 8u); }
    atomicAdd(&pool.activity[1], 1u);
}

// No current-frame draw has been encoded yet; prior-frame reads precede this
// pass in queue order. Reused descriptor slots must not inherit another Chunk.
@compute @workgroup_size(64)
fn reclaim_models(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let slot = invocation.x;
    if (slot >= selection.descriptor_count) { return; }
    if (slot == 0u) { atomicStore(&dispatch[0], 0u); }
    requests[slot].destination.x = INVALID;
    let model = models[slot];
    if (descriptors[slot].metadata.x != 0u && model.allocation.w == descriptors[slot].svo.w) { return; }
    if (model.generations.w != 0u) { release_page(model.allocation.x, model.allocation.y); }
    models[slot] = ModelState(vec4(0u), vec4(0u));
    for (var lod = 0u; lod < 5u; lod += 1u) { descriptors[slot].draws[lod] = vec4(0u); }
    descriptors[slot].metadata.z = 0u;
}

// A separate dispatch is the global synchronization between freeing old pages
// and assigning new classes. No allocator thread waits on another workgroup.
@compute @workgroup_size(64)
fn reclassify_slabs(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let slab = invocation.x;
    if (slab < selection.slab_count && atomicLoad(&pool.slabs[slab].live_pages) == 0u) {
        atomicStore(&pool.slabs[slab].page_class, 0u);
    }
}

fn claim_page(slab: u32, page_class: u32) -> u32 {
    let pages = 4096u >> page_class;
    // Allocation and reclamation are separate passes. A full slab cannot gain
    // space during this dispatch, so avoid scanning all of its bitmap words.
    if (atomicLoad(&pool.slabs[slab].live_pages) >= pages) { return INVALID; }
    let words = (pages + 31u) / 32u;
    for (var word = 0u; word < words; word += 1u) {
        var mask = 0xffffffffu;
        if (pages < 32u) { mask = (1u << pages) - 1u; }
        // Bits only transition free -> claimed during allocation. Each failed
        // atomicOr consumes a bit claimed by another invocation, bounding retries.
        for (var attempt = 0u; attempt < 32u; attempt += 1u) {
            let available = ~atomicLoad(&pool.slabs[slab].bitmap[word]) & mask;
            if (available == 0u) { break; }
            let bit = firstTrailingBit(available);
            let flag = 1u << bit;
            if ((atomicOr(&pool.slabs[slab].bitmap[word], flag) & flag) == 0u) {
                atomicAdd(&pool.slabs[slab].live_pages, 1u);
                atomicAdd(&pool.activity[0], 1u);
                return word * 32u + bit;
            }
        }
    }
    return INVALID;
}

fn allocate_page(page_class: u32) -> vec2<u32> {
    // First fit within the smallest suitable class. Reuse assigned slabs before
    // taking shared capacity for a class; never silently promote to a larger page.
    for (var phase = 0u; phase < 2u; phase += 1u) {
        for (var slab = 0u; slab < selection.slab_count; slab += 1u) {
            var assigned = atomicLoad(&pool.slabs[slab].page_class);
            if (phase == 1u && assigned == 0u) {
                let claimed = atomicCompareExchangeWeak(&pool.slabs[slab].page_class, 0u, page_class + 1u);
                assigned = claimed.old_value;
                if (claimed.exchanged) { assigned = page_class + 1u; }
                // Retry a spurious weak-CAS failure only while unassigned. This
                // is not a lock: no invocation waits for an owner to release it.
                while (assigned == 0u) {
                    let retry = atomicCompareExchangeWeak(&pool.slabs[slab].page_class, 0u, page_class + 1u);
                    assigned = retry.old_value;
                    if (retry.exchanged) { assigned = page_class + 1u; }
                }
            }
            if (assigned == page_class + 1u) {
                let page = claim_page(slab, page_class);
                if (page != INVALID) { return vec2(slab, page); }
            }
        }
    }
    return vec2(INVALID);
}

fn projected_error(matrix: mat4x4<f32>, viewport: vec4<u32>, error: f32) -> f32 {
    if (error == 0.0) { return 0.0; }
    var clips: array<vec4<f32>, 8>;
    var behind = true;
    for (var i = 0u; i < 8u; i += 1u) {
        let p = vec3<f32>(f32(i & 1u), f32((i >> 1u) & 1u), f32((i >> 2u) & 1u)) * 16.0;
        clips[i] = matrix * vec4(p, 1.0);
        behind = behind && clips[i].w < 0.0;
    }
    let sign = select(1.0, -1.0, behind);
    var min_w = bitcast<f32>(0x7f7fffffu);
    var ndc = vec2(0.0);
    for (var i = 0u; i < 8u; i += 1u) {
        let clip = clips[i] * sign;
        if (!all(abs(clip) <= vec4(bitcast<f32>(0x7f7fffffu))) || clip.w <= 0.0 || (sign > 0.0 && clip.z >= clip.w)) {
            return bitcast<f32>(0x7f800000u);
        }
        min_w = min(min_w, clip.w);
        ndc = max(ndc, abs(clip.xy) / clip.w);
    }
    let delta_w = length(vec3(matrix[0].w, matrix[1].w, matrix[2].w)) * error;
    let denominator = min_w - delta_w;
    if (denominator <= 0.0) { return bitcast<f32>(0x7f800000u); }
    let dx = length(vec3(matrix[0].x, matrix[1].x, matrix[2].x)) * error;
    let dy = length(vec3(matrix[0].y, matrix[1].y, matrix[2].y)) * error;
    return length((vec2(dx, dy) + ndc * delta_w) * vec2<f32>(viewport.zw) * 0.5 / denominator);
}

@compute @workgroup_size(64)
fn select_models(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let slot = invocation.x;
    if (slot >= selection.descriptor_count) { return; }
    let descriptor = descriptors[slot];
    let model = models[slot];
    if (descriptor.metadata.x == 0u || model.generations.x != descriptor.metadata.w || atomicLoad(&pool.fault[0]) != 0u) { return; }
    var lod = 0u;
    var count = atomicLoad(&measurements[slot * 5u].count);
    var tolerance = 0.0;
    if (selection.tolerance >= 0.0 && selection.tolerance <= bitcast<f32>(0x7f7fffffu)) { tolerance = selection.tolerance; }
    for (var candidate = 1u; candidate < 5u; candidate += 1u) {
        let candidate_count = atomicLoad(&measurements[slot * 5u + candidate].count);
        if (candidate_count >= count) { continue; }
        let error = bitcast<f32>(atomicLoad(&measurements[slot * 5u + candidate].error_bits));
        var acceptable = true;
        for (var view = 0u; view < selection.view_count; view += 1u) {
            acceptable = acceptable && projected_error(views[view].clip_from_world * descriptor.world_from_chunk, views[view].viewport, error) <= tolerance;
        }
        if (acceptable) { lod = candidate; count = candidate_count; }
    }
    if (model.generations.y == descriptor.metadata.w && model.generations.z == lod) { return; }
    if (count == 0u) {
        let index = atomicAdd(&dispatch[0], 1u);
        requests[index] = Request(vec4(slot, lod, 0u, 0u), vec4(0u, 0u, INVALID, INVALID));
        return;
    }
    var page_class = 0u;
    var capacity = MIN_PAGE_RECORDS;
    while (capacity < count && capacity < SLAB_RECORDS) { capacity *= 2u; page_class += 1u; }
    var allocation = vec2(INVALID);
    if (capacity >= count) { allocation = allocate_page(page_class); }
    if (allocation.x == INVALID) {
        atomicOr(&pool.fault[0], 1u);
        atomicStore(&pool.fault[1], slot);
        atomicStore(&pool.fault[2], count * 16u);
        atomicStore(&pool.fault[3], page_class);
        return;
    }
    let location = pool.slabs[allocation.x].location;
    let index = atomicAdd(&dispatch[0], 1u);
    requests[index] = Request(
        vec4(slot, lod, location.x + allocation.y * capacity, location.y),
        vec4(count, capacity, allocation.x, allocation.y),
    );
}

@compute @workgroup_size(1)
fn finalize_dispatch() {
    let count = atomicLoad(&dispatch[0]);
    atomicStore(&dispatch[1], 96u + 512u + 64u + 8u + 1u);
    atomicStore(&dispatch[2], min(count, 64u));
    atomicStore(&dispatch[3], (count + 63u) / 64u);
    atomicStore(&dispatch[4], (count + 63u) / 64u);
    atomicStore(&dispatch[5], 1u);
    atomicStore(&dispatch[6], 1u);
}
