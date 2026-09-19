struct PackedSvoNode {
    first_child: u32,
    child_mask: u32,
    data: u32,
    reserved: u32,
}

struct GeneratedQuad {
    data: vec4<u32>,
}

struct ChunkDescriptor {
    world_from_chunk: mat4x4<f32>,
    chunk_coordinate: vec4<i32>,
    draws: array<vec4<u32>, 5>,
    // active, material seed, selected LOD, source generation
    metadata: vec4<u32>,
    // node offset, node count, SVO segment, dirty generation (zero = clean)
    svo: vec4<u32>,
    // Six neighbor slots packed into two vectors; 0xffffffff means absent.
    neighbor_slots: array<vec4<u32>, 2>,
}

struct BuildPass {
    svo_segment: u32,
    geometry_segment: u32,
    descriptor_count: u32,
    _reserved: u32,
}

@group(0) @binding(0) var<storage, read> packed_svos: array<PackedSvoNode>;
@group(0) @binding(1) var<storage, read_write> quads: array<GeneratedQuad>;
@group(0) @binding(2) var<storage, read_write> descriptors: array<ChunkDescriptor>;
@group(0) @binding(3) var<storage, read_write> quad_counts: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> boundary_rows: array<u32>;
@group(0) @binding(5) var<uniform> build_pass: BuildPass;
@group(0) @binding(6) var<storage, read> build_slots: array<u32>;

var<workgroup> visible_rows: array<u32, 16>;

fn occupied_cell(descriptor: ChunkDescriptor, cell: vec3<u32>, lod: u32) -> bool {
    let finest = cell << vec3<u32>(lod);
    let target_depth = 4u - lod;
    var index = descriptor.svo.x;
    var node = packed_svos[index];
    for (var depth = 0u; depth < target_depth; depth += 1u) {
        let bit = 3u - depth;
        let octant = ((finest.x >> bit) & 1u)
            | (((finest.y >> bit) & 1u) << 1u)
            | (((finest.z >> bit) & 1u) << 2u);
        let octant_bit = 1u << octant;
        if ((node.child_mask & octant_bit) == 0u) {
            return node.data != 0u;
        }
        let preceding = countOneBits(node.child_mask & (octant_bit - 1u));
        index = descriptor.svo.x + node.first_child + preceding;
        node = packed_svos[index];
    }
    return (node.reserved & 1u) != 0u;
}

fn grid_size(lod: u32) -> vec3<u32> {
    return vec3(16u >> lod);
}

fn neighbor_slot(descriptor: ChunkDescriptor, face: u32) -> u32 {
    return descriptor.neighbor_slots[face / 4u][face % 4u];
}

fn neighbor_occupied(descriptor: ChunkDescriptor, face: u32, coordinate: vec3<u32>) -> bool {
    let slot = neighbor_slot(descriptor, face);
    if (slot == 0xffffffffu) { return false; }
    var u: u32;
    var v: u32;
    if (face < 2u) {
        u = coordinate.y;
        v = coordinate.z;
    } else if (face < 4u) {
        u = coordinate.x;
        v = coordinate.z;
    } else {
        u = coordinate.x;
        v = coordinate.y;
    }
    // Opposing faces are adjacent: +x reads the neighbor's -x face, etc.
    let opposite = face ^ 1u;
    let row = boundary_rows[slot * 96u + opposite * 16u + v];
    return (row & (1u << u)) != 0u;
}

fn face_cell(face: u32, slice: u32, u: u32, v: u32) -> vec3<u32> {
    if (face < 2u) { return vec3(slice, u, v); }
    if (face < 4u) { return vec3(u, slice, v); }
    return vec3(u, v, slice);
}

fn face_plane_size(face: u32, sizes: vec3<u32>) -> vec3<u32> {
    if (face < 2u) { return vec3(sizes.x, sizes.y, sizes.z); }
    if (face < 4u) { return vec3(sizes.y, sizes.x, sizes.z); }
    return vec3(sizes.z, sizes.x, sizes.y);
}

fn face_offset(face: u32) -> vec3<i32> {
    switch face {
        case 0u: { return vec3(1, 0, 0); }
        case 1u: { return vec3(-1, 0, 0); }
        case 2u: { return vec3(0, 1, 0); }
        case 3u: { return vec3(0, -1, 0); }
        case 4u: { return vec3(0, 0, 1); }
        default: { return vec3(0, 0, -1); }
    }
}

fn face_visible(descriptor: ChunkDescriptor, face: u32, cell: vec3<u32>, sizes: vec3<u32>, lod: u32) -> bool {
    if (!occupied_cell(descriptor, cell, lod)) { return false; }
    let neighbor = vec3<i32>(cell) + face_offset(face);
    if (any(neighbor < vec3(0)) || any(neighbor >= vec3<i32>(sizes))) {
        var neighbor_cell = neighbor;
        for (var axis = 0u; axis < 3u; axis += 1u) {
            if (neighbor_cell[axis] < 0) {
                neighbor_cell[axis] = 15;
            } else if (neighbor_cell[axis] >= i32(sizes[axis])) {
                neighbor_cell[axis] = 0;
            }
        }
        return !neighbor_occupied(descriptor, face, vec3<u32>(neighbor_cell));
    }
    return !occupied_cell(descriptor, vec3<u32>(neighbor), lod);
}

fn is_positive_face(face: u32) -> bool {
    return face == 0u || face == 2u || face == 4u;
}

fn maximum_quads(lod: u32) -> u32 {
    let grid = 16u >> lod;
    return 3u * grid * grid * grid + 768u;
}

fn emit_quad(slot: u32, descriptor: ChunkDescriptor, face: u32, slice: u32, u: u32, v: u32, width: u32, height: u32, lod: u32) {
    let local_index = atomicAdd(&quad_counts[slot * 5u + lod], 1u);
    if (local_index >= maximum_quads(lod)) { return; }
    let draw = descriptor.draws[lod];
    let quad_index = draw.y / 6u + local_index;
    quads[quad_index].data = vec4(
        face,
        slice,
        u | (v << 8u),
        width | (height << 8u) | (lod << 16u),
    );
}

@compute @workgroup_size(1)
fn prepare_boundary(@builtin(workgroup_id) workgroup: vec3<u32>) {
    let build_index = workgroup.y;
    if (build_index >= build_pass.descriptor_count) { return; }
    let slot = build_slots[build_index];
    if (slot == 0xffffffffu) { return; }
    let plane = workgroup.x;
    let descriptor = descriptors[slot];
    if (descriptor.metadata.x == 0u || descriptor.svo.w == 0u || descriptor.svo.z != build_pass.svo_segment) { return; }
    let face = plane / 16u;
    let v = plane % 16u;
    let slice = select(0u, 15u, is_positive_face(face));
    var row = 0u;
    for (var u = 0u; u < 16u; u += 1u) {
        if (occupied_cell(descriptor, face_cell(face, slice, u, v), 0u)) {
            row |= 1u << u;
        }
    }
    boundary_rows[slot * 96u + plane] = row;
}

@compute @workgroup_size(64)
fn reset_geometry(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let build_index = invocation.x;
    if (build_index >= build_pass.descriptor_count) { return; }
    let slot = build_slots[build_index];
    if (slot == 0xffffffffu) { return; }
    let descriptor = descriptors[slot];
    if (descriptor.metadata.x == 0u || descriptor.svo.w == 0u || descriptor.svo.z != build_pass.svo_segment) { return; }
    for (var lod = 0u; lod < 5u; lod += 1u) {
        if (descriptor.draws[lod].w != 0u && descriptor.draws[lod].z == build_pass.geometry_segment) {
            atomicStore(&quad_counts[slot * 5u + lod], 0u);
            descriptors[slot].draws[lod].x = 0u;
        }
    }
}

@compute @workgroup_size(1)
fn mesh_chunks(@builtin(workgroup_id) workgroup: vec3<u32>) {
    let planes_per_chunk = 5u * 96u;
    let build_index = workgroup.y;
    if (build_index >= build_pass.descriptor_count) { return; }
    let slot = build_slots[build_index];
    if (slot == 0xffffffffu) { return; }
    let within_chunk = workgroup.x;
    let self_lod = within_chunk / 96u;
    let plane = within_chunk % 96u;
    let descriptor = descriptors[slot];
    if (descriptor.metadata.x == 0u || descriptor.svo.w == 0u || descriptor.svo.z != build_pass.svo_segment) { return; }
    if (descriptor.draws[self_lod].w == 0u || descriptor.draws[self_lod].z != build_pass.geometry_segment) { return; }

    let self_plane_size = face_plane_size(plane / 16u, grid_size(self_lod));
    let face = plane / 16u;
    let self_slice = plane % 16u;
    if (self_slice >= self_plane_size.x) { return; }
    let outer = select(self_slice == 0u, self_slice + 1u == self_plane_size.x, is_positive_face(face));
    let lod = select(self_lod, 0u, outer && neighbor_slot(descriptor, face) != 0xffffffffu);
    let plane_size = face_plane_size(face, grid_size(lod));
    let slice = select(self_slice, select(0u, plane_size.x - 1u, is_positive_face(face)), outer);

    for (var v = 0u; v < plane_size.z; v += 1u) {
        var row = 0u;
        for (var u = 0u; u < plane_size.y; u += 1u) {
            if (face_visible(descriptor, face, face_cell(face, slice, u, v), grid_size(lod), lod)) {
                row |= 1u << u;
            }
        }
        visible_rows[v] = row;
    }

    for (var v = 0u; v < plane_size.z; v += 1u) {
        var u = 0u;
        while (u < plane_size.y) {
            let bit = 1u << u;
            if ((visible_rows[v] & bit) == 0u) { u += 1u; continue; }
            var width = 1u;
            while (u + width < plane_size.y && (visible_rows[v] & (1u << (u + width))) != 0u) { width += 1u; }
            let run_mask = ((1u << width) - 1u) << u;
            var height = 1u;
            while (v + height < plane_size.z && (visible_rows[v + height] & run_mask) == run_mask) { height += 1u; }
            for (var clear_v = v; clear_v < v + height; clear_v += 1u) {
                visible_rows[clear_v] &= ~run_mask;
            }
            emit_quad(slot, descriptor, face, slice, u, v, width, height, lod);
            u += width;
        }
    }
}

@compute @workgroup_size(64)
fn finalize_builds(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let build_index = invocation.x;
    if (build_index >= build_pass.descriptor_count) { return; }
    let slot = build_slots[build_index];
    if (slot == 0xffffffffu) { return; }
    if (descriptors[slot].metadata.x != 0u && descriptors[slot].svo.z == build_pass.svo_segment) {
        for (var lod = 0u; lod < 5u; lod += 1u) {
            if (descriptors[slot].draws[lod].w != 0u) {
                descriptors[slot].draws[lod].x = atomicLoad(&quad_counts[slot * 5u + lod]) * 6u;
            }
        }
        descriptors[slot].svo.w = 0u;
    }
}
