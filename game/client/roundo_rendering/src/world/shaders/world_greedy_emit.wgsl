struct GeneratedVertex {
    position_and_face: vec4<f32>,
}

struct DrawIndirect {
    vertex_count: atomic<u32>,
    instance_count: atomic<u32>,
    first_vertex: atomic<u32>,
    first_instance: atomic<u32>,
}

struct ChunkUniform {
    world_from_chunk: mat4x4<f32>,
    // root index, maximum depth, vertex capacity, material seed
    svo: vec4<u32>,
    chunk_coordinate: vec4<i32>,
    // root, depth, available, desired LOD; order is +x,-x,+y,-y,+z,-z.
    neighbor_svo: array<vec4<u32>, 6>,
}

// Each occupancy buffer contains the complete bit-packed cubic mip chain
// 16³ → 8³ → 4³ → 2³ → 1³. Offsets are measured in bits.
@group(0) @binding(0) var<storage, read> occupancy_mip: array<u32>;
@group(0) @binding(1) var<storage, read_write> vertices: array<GeneratedVertex>;
@group(0) @binding(2) var<storage, read_write> draw: DrawIndirect;
@group(0) @binding(3) var<uniform> chunk: ChunkUniform;
@group(0) @binding(4) var<storage, read> neighbor_chunks_pos_x: array<u32>;
@group(0) @binding(5) var<storage, read> neighbor_chunks_neg_x: array<u32>;
@group(0) @binding(6) var<storage, read> neighbor_chunks_pos_y: array<u32>;
@group(0) @binding(7) var<storage, read> neighbor_chunks_neg_y: array<u32>;
@group(0) @binding(8) var<storage, read> neighbor_chunks_pos_z: array<u32>;
@group(0) @binding(9) var<storage, read> neighbor_chunks_neg_z: array<u32>;

const OCCUPANCY_MIP_OFFSETS: array<u32, 5> = array<u32, 5>(0u, 4096u, 4608u, 4672u, 4680u);

var<workgroup> visible_rows: array<u32, 16>;

fn occupancy_word(source: u32, index: u32) -> u32 {
    switch source {
        case 0u: { return occupancy_mip[index]; }
        case 1u: { return neighbor_chunks_pos_x[index]; }
        case 2u: { return neighbor_chunks_neg_x[index]; }
        case 3u: { return neighbor_chunks_pos_y[index]; }
        case 4u: { return neighbor_chunks_neg_y[index]; }
        case 5u: { return neighbor_chunks_pos_z[index]; }
        default: { return neighbor_chunks_neg_z[index]; }
    }
}

fn occupied_cell(source: u32, cell: vec3<u32>, lod: u32) -> bool {
    let edge = 16u >> lod;
    let bit_index = OCCUPANCY_MIP_OFFSETS[lod]
        + (cell.z * edge + cell.y) * edge
        + cell.x;
    return (occupancy_word(source, bit_index >> 5u) & (1u << (bit_index & 31u))) != 0u;
}

fn grid_size(lod: u32) -> vec3<u32> {
    return vec3(16u >> lod);
}

fn neighbor_occupied(face: u32, coordinate: vec3<u32>, lod: u32) -> bool {
    if (chunk.neighbor_svo[face].z == 0u) {
        return false;
    }
    return occupied_cell(face + 1u, coordinate, lod);
}

fn face_cell(face: u32, slice: u32, u: u32, v: u32) -> vec3<u32> {
    if (face < 2u) {
        return vec3(slice, u, v);
    }
    if (face < 4u) {
        return vec3(u, slice, v);
    }
    return vec3(u, v, slice);
}

// x = slice axis, y = row bit axis, z = row array axis.
fn face_plane_size(face: u32, sizes: vec3<u32>) -> vec3<u32> {
    if (face < 2u) {
        return vec3(sizes.x, sizes.y, sizes.z);
    }
    if (face < 4u) {
        return vec3(sizes.y, sizes.x, sizes.z);
    }
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

fn face_visible(face: u32, cell: vec3<u32>, sizes: vec3<u32>, lod: u32) -> bool {
    if (!occupied_cell(0u, cell, lod)) {
        return false;
    }
    let neighbor = vec3<i32>(cell) + face_offset(face);
    if (any(neighbor < vec3(0)) || any(neighbor >= vec3<i32>(sizes))) {
        var neighbor_cell = neighbor;
        for (var axis = 0u; axis < 3u; axis += 1u) {
            if (neighbor_cell[axis] < 0) {
                neighbor_cell[axis] = i32(sizes[axis]) - 1;
            } else if (neighbor_cell[axis] >= i32(sizes[axis])) {
                neighbor_cell[axis] = 0;
            }
        }
        return !neighbor_occupied(face, vec3<u32>(neighbor_cell), lod);
    }
    return !occupied_cell(0u, vec3<u32>(neighbor), lod);
}

fn face_corner(face: u32, slice: u32, u: f32, v: f32, lod: u32) -> vec3<f32> {
    let s = f32(slice);
    let cell_size = f32(1u << lod);
    var corner: vec3<f32>;
    switch face {
        case 0u: { corner = vec3(s + 1.0, u, v); }
        case 1u: { corner = vec3(s, u, v); }
        case 2u: { corner = vec3(u, s + 1.0, v); }
        case 3u: { corner = vec3(u, s, v); }
        case 4u: { corner = vec3(u, v, s + 1.0); }
        default: { corner = vec3(u, v, s); }
    }
    return corner * cell_size;
}

fn write_vertex(index: u32, position: vec3<f32>, face: u32, lod: u32) {
    vertices[index].position_and_face = vec4(position, f32(face + lod * 8u));
}

fn emit_quad(face: u32, slice: u32, u: u32, v: u32, width: u32, height: u32, lod: u32) {
    let first = atomicAdd(&draw.vertex_count, 6u);
    if (first + 6u > chunk.svo.z) {
        return;
    }
    let u0 = f32(u);
    let v0 = f32(v);
    let u1 = f32(u + width);
    let v1 = f32(v + height);
    let a = face_corner(face, slice, u0, v0, lod);
    let b = face_corner(face, slice, u1, v0, lod);
    let c = face_corner(face, slice, u1, v1, lod);
    let d = face_corner(face, slice, u0, v1, lod);

    if (face == 0u || face == 3u || face == 4u) {
        write_vertex(first + 0u, a, face, lod);
        write_vertex(first + 1u, b, face, lod);
        write_vertex(first + 2u, c, face, lod);
        write_vertex(first + 3u, a, face, lod);
        write_vertex(first + 4u, c, face, lod);
        write_vertex(first + 5u, d, face, lod);
    } else {
        write_vertex(first + 0u, a, face, lod);
        write_vertex(first + 1u, c, face, lod);
        write_vertex(first + 2u, b, face, lod);
        write_vertex(first + 3u, a, face, lod);
        write_vertex(first + 4u, d, face, lod);
        write_vertex(first + 5u, c, face, lod);
    }
}

fn is_positive_face(face: u32) -> bool {
    return face == 0u || face == 2u || face == 4u;
}

@compute @workgroup_size(1)
fn mesh_chunk(@builtin(workgroup_id) workgroup: vec3<u32>) {
    let self_lod = u32(chunk.chunk_coordinate.w);
    let self_plane_size = face_plane_size(workgroup.x / 16u, grid_size(self_lod));
    let face = workgroup.x / 16u;
    let self_slice = workgroup.x % 16u;
    if (self_slice >= self_plane_size.x) {
        return;
    }

    // Only an outer face changes resolution. Interior planes remain at this
    // Chunk's selected LOD. Both sides choose min(self, neighbor), so the
    // boundary masks have identical Cell footprints even if one mesh is queued.
    let outer = select(self_slice == 0u, self_slice + 1u == self_plane_size.x, is_positive_face(face));
    let lod = select(self_lod, min(self_lod, chunk.neighbor_svo[face].w), outer && chunk.neighbor_svo[face].z != 0u);
    let plane_size = face_plane_size(face, grid_size(lod));
    let slice = select(self_slice, select(0u, plane_size.x - 1u, is_positive_face(face)), outer);

    for (var v = 0u; v < plane_size.z; v += 1u) {
        var row = 0u;
        for (var u = 0u; u < plane_size.y; u += 1u) {
            if (face_visible(face, face_cell(face, slice, u, v), grid_size(lod), lod)) {
                row |= 1u << u;
            }
        }
        visible_rows[v] = row;
    }

    for (var v = 0u; v < plane_size.z; v += 1u) {
        var u = 0u;
        while (u < plane_size.y) {
            let bit = 1u << u;
            if ((visible_rows[v] & bit) == 0u) {
                u += 1u;
                continue;
            }

            var width = 1u;
            while (u + width < plane_size.y && (visible_rows[v] & (1u << (u + width))) != 0u) {
                width += 1u;
            }
            let run_mask = ((1u << width) - 1u) << u;
            var height = 1u;
            while (v + height < plane_size.z && (visible_rows[v + height] & run_mask) == run_mask) {
                height += 1u;
            }
            for (var clear_v = v; clear_v < v + height; clear_v += 1u) {
                visible_rows[clear_v] &= ~run_mask;
            }
            emit_quad(face, slice, u, v, width, height, lod);
            u += width;
        }
    }
}
