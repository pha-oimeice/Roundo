struct PackedSvoNode {
    first_child: u32,
    child_mask: u32,
    data: u32,
    reserved: u32,
}

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
    // root index, maximum depth, available, reserved; order is +x,-x,+y,-y,+z,-z.
    neighbor_svo: array<vec4<u32>, 6>,
}

@group(0) @binding(0) var<storage, read> nodes: array<PackedSvoNode>;
@group(0) @binding(1) var<storage, read_write> vertices: array<GeneratedVertex>;
@group(0) @binding(2) var<storage, read_write> draw: DrawIndirect;
@group(0) @binding(3) var<uniform> chunk: ChunkUniform;
@group(0) @binding(4) var<storage, read> neighbor_chunks_pos_x: array<PackedSvoNode>;
@group(0) @binding(5) var<storage, read> neighbor_chunks_neg_x: array<PackedSvoNode>;
@group(0) @binding(6) var<storage, read> neighbor_chunks_pos_y: array<PackedSvoNode>;
@group(0) @binding(7) var<storage, read> neighbor_chunks_neg_y: array<PackedSvoNode>;
@group(0) @binding(8) var<storage, read> neighbor_chunks_pos_z: array<PackedSvoNode>;
@group(0) @binding(9) var<storage, read> neighbor_chunks_neg_z: array<PackedSvoNode>;

var<workgroup> visible_rows: array<u32, 16>;

fn child_index(node: PackedSvoNode, octant: u32) -> u32 {
    let octant_bit = 1u << octant;
    let preceding = countOneBits(node.child_mask & (octant_bit - 1u));
    return node.first_child + preceding;
}

fn svo_node(source: u32, index: u32) -> PackedSvoNode {
    switch source {
        case 0u: { return nodes[index]; }
        case 1u: { return neighbor_chunks_pos_x[index]; }
        case 2u: { return neighbor_chunks_neg_x[index]; }
        case 3u: { return neighbor_chunks_pos_y[index]; }
        case 4u: { return neighbor_chunks_neg_y[index]; }
        case 5u: { return neighbor_chunks_pos_z[index]; }
        default: { return neighbor_chunks_neg_z[index]; }
    }
}

fn occupied_in_svo(
    source: u32,
    coordinate: vec3<u32>,
    lod: u32,
    metadata: vec4<u32>,
) -> bool {
    let target_depth = metadata.y - lod;
    var node_index = metadata.x;
    for (var depth = 0u; depth < target_depth; depth += 1u) {
        let node = svo_node(source, node_index);
        let bit = target_depth - depth - 1u;
        let octant = ((coordinate.x >> bit) & 1u)
            | (((coordinate.y >> bit) & 1u) << 1u)
            | (((coordinate.z >> bit) & 1u) << 2u);
        let octant_bit = 1u << octant;
        if ((node.child_mask & octant_bit) == 0u) {
            return node.data != 0u;
        }
        node_index = child_index(node, octant);
    }
    // Packed bit zero conservatively preserves any occupied descendant surface.
    return ((svo_node(source, node_index).reserved & 1u) != 0u);
}

fn occupied_at_lod(coordinate: vec3<u32>, lod: u32) -> bool {
    return occupied_in_svo(0u, coordinate, lod, chunk.svo);
}

fn neighbor_occupied(face: u32, coordinate: vec3<u32>, lod: u32) -> bool {
    let metadata = chunk.neighbor_svo[face];
    if (metadata.z == 0u) {
        return false;
    }
    return occupied_in_svo(face + 1u, coordinate, lod, metadata);
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

fn face_visible(face: u32, cell: vec3<u32>, grid_size: u32, lod: u32) -> bool {
    if (!occupied_at_lod(cell, lod)) {
        return false;
    }
    let neighbor = vec3<i32>(cell) + face_offset(face);
    if (any(neighbor < vec3(0)) || any(neighbor >= vec3<i32>(i32(grid_size)))) {
        var wrapped = neighbor;
        for (var axis = 0u; axis < 3u; axis += 1u) {
            if (wrapped[axis] < 0) {
                wrapped[axis] = i32(grid_size) - 1;
            } else if (wrapped[axis] >= i32(grid_size)) {
                wrapped[axis] = 0;
            }
        }
        return !neighbor_occupied(face, vec3<u32>(wrapped), lod);
    }
    return !occupied_at_lod(vec3<u32>(neighbor), lod);
}

fn face_corner(face: u32, slice: u32, u: f32, v: f32, cell_size: f32) -> vec3<f32> {
    let s = f32(slice);
    switch face {
        case 0u: { return vec3(s + 1.0, u, v) * cell_size; }
        case 1u: { return vec3(s, u, v) * cell_size; }
        case 2u: { return vec3(u, s + 1.0, v) * cell_size; }
        case 3u: { return vec3(u, s, v) * cell_size; }
        case 4u: { return vec3(u, v, s + 1.0) * cell_size; }
        default: { return vec3(u, v, s) * cell_size; }
    }
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
    let cell_size = f32(1u << lod);
    let a = face_corner(face, slice, u0, v0, cell_size);
    let b = face_corner(face, slice, u1, v0, cell_size);
    let c = face_corner(face, slice, u1, v1, cell_size);
    let d = face_corner(face, slice, u0, v1, cell_size);

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

@compute @workgroup_size(1)
fn mesh_chunk(@builtin(workgroup_id) workgroup: vec3<u32>) {
    atomicStore(&draw.instance_count, 1u);
    let lod = u32(chunk.chunk_coordinate.w);
    let grid_size = 16u >> lod;
    let face = workgroup.x / 16u;
    let slice = workgroup.x % 16u;
    if (slice >= grid_size) {
        return;
    }

    for (var v = 0u; v < grid_size; v += 1u) {
        var row = 0u;
        for (var u = 0u; u < grid_size; u += 1u) {
            if (face_visible(face, face_cell(face, slice, u, v), grid_size, lod)) {
                row |= 1u << u;
            }
        }
        visible_rows[v] = row;
    }

    for (var v = 0u; v < grid_size; v += 1u) {
        var u = 0u;
        while (u < grid_size) {
            let bit = 1u << u;
            if ((visible_rows[v] & bit) == 0u) {
                u += 1u;
                continue;
            }

            var width = 1u;
            while (u + width < grid_size && (visible_rows[v] & (1u << (u + width))) != 0u) {
                width += 1u;
            }
            let run_mask = ((1u << width) - 1u) << u;
            var height = 1u;
            while (v + height < grid_size && (visible_rows[v + height] & run_mask) == run_mask) {
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
