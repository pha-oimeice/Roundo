// Canonical surface measurement and generation use the SAME mesher. Higher
// levels replace only manifold disk patches whose projection is convex. Patch
// boundaries remain exact; all other regions use exact clipped greedy quads.
struct PackedSvoNode { first_child: u32, child_mask: u32, data: u32, reserved: u32 }
struct GeneratedQuad { data: vec4<u32> }
struct ChunkDescriptor {
    world_from_chunk: mat4x4<f32>, chunk_coordinate: vec4<i32>,
    draws: array<vec4<u32>, 5>, metadata: vec4<u32>, svo: vec4<u32>, neighbor_slots: array<vec4<u32>, 2>,
}
struct BuildPass { svo_segment: u32, geometry_segment: u32, descriptor_count: u32, reserved: u32 }
struct Request { destination: vec4<u32>, bounds: vec4<u32> }
struct Measurement { count: atomic<u32>, error_bits: atomic<u32>, reserved0: u32, reserved1: u32 }
struct ModelState { generations: vec4<u32>, allocation: vec4<u32> }
// Page ABI shared with arena.rs and world_model_select.wgsl.
struct PageSlab {
    location: vec4<u32>, page_class: atomic<u32>, live_pages: atomic<u32>, reserved0: u32, reserved1: u32,
    bitmap: array<atomic<u32>, 128>,
}
struct PagePool { fault: array<atomic<u32>, 4>, activity: array<atomic<u32>, 4>, slabs: array<PageSlab> }
@group(0) @binding(0) var<storage, read> packed_svos: array<PackedSvoNode>;
@group(0) @binding(1) var<storage, read_write> quads: array<GeneratedQuad>;
@group(0) @binding(2) var<storage, read_write> descriptors: array<ChunkDescriptor>;
@group(0) @binding(3) var<storage, read_write> measurements: array<Measurement>;
@group(0) @binding(4) var<storage, read_write> boundary_rows: array<u32>;
@group(0) @binding(5) var<uniform> build_pass: BuildPass;
@group(0) @binding(6) var<storage, read_write> requests: array<Request>;
@group(0) @binding(7) var<storage, read_write> models: array<ModelState>;
@group(0) @binding(8) var<storage, read_write> pool: PagePool;
var<workgroup> rows: array<u32, 16>;
// A monotone surface intersects each of six box faces in at most two edge-length
// paths. Overflow rejects the approximation rather than truncating its boundary.
var<workgroup> edges: array<vec2<u32>, 192>;
var<workgroup> contour: array<vec3<i32>, 192>;

fn occupied(descriptor: ChunkDescriptor, cell: vec3<i32>) -> bool {
    if (any(cell < vec3(0)) || any(cell >= vec3(16))) {
        var face = 0u;
        var u = 0i; var v = 0i;
        if (cell.x < 0 || cell.x >= 16) { face = select(1u, 0u, cell.x >= 16); u = cell.y; v = cell.z; }
        else if (cell.y < 0 || cell.y >= 16) { face = select(3u, 2u, cell.y >= 16); u = cell.x; v = cell.z; }
        else { face = select(5u, 4u, cell.z >= 16); u = cell.x; v = cell.y; }
        if (u < 0 || u >= 16 || v < 0 || v >= 16) { return false; }
        let slot = descriptor.neighbor_slots[face / 4u][face % 4u];
        if (slot == 0xffffffffu) { return false; }
        return (boundary_rows[slot * 96u + (face ^ 1u) * 16u + u32(v)] & (1u << u32(u))) != 0u;
    }
    var node = packed_svos[descriptor.svo.x];
    for (var depth = 0u; depth < 4u; depth += 1u) {
        let bit = 3u - depth;
        let p = vec3<u32>(cell);
        let octant = ((p.x >> bit) & 1u) | (((p.y >> bit) & 1u) << 1u) | (((p.z >> bit) & 1u) << 2u);
        let flag = 1u << octant;
        if ((node.child_mask & flag) == 0u) { return node.data != 0u; }
        node = packed_svos[descriptor.svo.x + node.first_child + countOneBits(node.child_mask & (flag - 1u))];
    }
    return node.data != 0u;
}
fn face_cell(face: u32, slice: i32, u: i32, v: i32) -> vec3<i32> {
    if (face < 2u) { return vec3(slice, u, v); }
    if (face < 4u) { return vec3(u, slice, v); }
    return vec3(u, v, slice);
}
fn normal(face: u32) -> vec3<i32> {
    var n = vec3(0); n[face / 2u] = select(-1, 1, (face & 1u) == 0u); return n;
}
fn visible(descriptor: ChunkDescriptor, face: u32, cell: vec3<i32>) -> bool {
    return occupied(descriptor, cell) && !occupied(descriptor, cell + normal(face));
}
fn pack_point(p: vec3<i32>) -> u32 { return u32(p.x) | (u32(p.y) << 5u) | (u32(p.z) << 10u); }
fn unpack_point(p: u32) -> vec3<i32> { return vec3<i32>(i32(p & 31u), i32((p >> 5u) & 31u), i32((p >> 10u) & 31u)); }
fn inside(p: vec3<i32>, low: vec3<i32>, size: i32) -> bool { return all(p >= low) && all(p < low + vec3(size)); }
// One canonical unit square at an axis-aligned grid plane. Only the solid cell
// owning the face may contribute it to this tile, so faces are never duplicated.
fn square(descriptor: ChunkDescriptor, axis: u32, p: vec3<i32>, low: vec3<i32>, size: i32) -> bool {
    var before = p; before[axis] -= 1;
    let a = occupied(descriptor, before); let b = occupied(descriptor, p);
    return (a && !b && inside(before, low, size)) || (b && !a && inside(p, low, size));
}
fn edge_count(descriptor: ChunkDescriptor, a: vec3<i32>, b: vec3<i32>, low: vec3<i32>, size: i32) -> u32 {
    var axis = 0u;
    if (a.y != b.y) { axis = 1u; } else if (a.z != b.z) { axis = 2u; }
    let u = (axis + 1u) % 3u; let v = (axis + 2u) % 3u;
    let p = min(a, b);
    var count = 0u;
    for (var side = -1; side <= 0; side += 1) {
        var q = p; q[v] += side;
        count += u32(square(descriptor, u, q, low, size));
        q = p; q[u] += side;
        count += u32(square(descriptor, v, q, low, size));
    }
    return count;
}
// The link of each surface vertex must be exactly one circle or one interval.
// This rejects edge/vertex contacts and disconnected fans, including thin-feature
// ambiguities that an occupancy ratio or QEF residual cannot detect.
fn manifold_vertex(descriptor: ChunkDescriptor, p: vec3<i32>, low: vec3<i32>, size: i32) -> bool {
    var graph = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
    for (var axis = 0u; axis < 3u; axis += 1u) {
        let u = (axis + 1u) % 3u; let v = (axis + 2u) % 3u;
        for (var i = -1; i <= 0; i += 1) { for (var j = -1; j <= 0; j += 1) {
            var q = p; q[u] += i; q[v] += j;
            if (square(descriptor, axis, q, low, size)) {
                let a = u * 2u + u32(i == 0); let b = v * 2u + u32(j == 0);
                graph[a] |= 1u << b; graph[b] |= 1u << a;
            }
        }}
    }
    var occupied_links = 0u; var ends = 0u;
    for (var i = 0u; i < 6u; i += 1u) {
        let degree = countOneBits(graph[i]);
        if (degree > 2u) { return false; }
        if (degree == 1u) { ends += 1u; }
        if (degree != 0u) { occupied_links |= 1u << i; }
    }
    if (occupied_links == 0u) { return true; }
    if (ends != 0u && ends != 2u) { return false; }
    // Every surface vertex on a tile face must belong to the retained contour.
    // Otherwise an interior fold/contact on that face could move away from an
    // adjacent tile's surface. No diagonal-neighbor guess is used to approve it.
    if ((any(p == low) || any(p == low + vec3(size))) && ends != 2u) { return false; }
    var reached = 1u << firstTrailingBit(occupied_links);
    for (var iteration = 0u; iteration < 6u; iteration += 1u) {
        for (var i = 0u; i < 6u; i += 1u) { if ((reached & (1u << i)) != 0u) { reached |= graph[i]; } }
    }
    return reached == occupied_links;
}

struct Patch { count: u32, error: f32, center: vec3<f32> }
fn no_patch() -> Patch { return Patch(0u, 0.0, vec3(0.0)); }
fn make_proxy(descriptor: ChunkDescriptor, low: vec3<i32>, size: i32) -> Patch {
    var flags = 0u; var boundary_count = 0u; var faces = 0u;
    var plane_counts = vec3(0.0); var plane_sums = vec3(0.0); var mass = vec3(0.0);
    var direction = vec3(0.0);
    for (var z = low.z; z < low.z + size; z += 1) { for (var y = low.y; y < low.y + size; y += 1) { for (var x = low.x; x < low.x + size; x += 1) {
        let cell = vec3(x, y, z);
        if (!occupied(descriptor, cell)) { continue; }
        for (var face = 0u; face < 6u; face += 1u) {
            if (!visible(descriptor, face, cell)) { continue; }
            flags |= 1u << face;
            // Opposing normals cannot form a single-valued patch. Preserve those
            // surfaces exactly, rather than filling cavities or deleting walls.
            if ((flags & (flags >> 1u) & 0x15u) != 0u) { return no_patch(); }
            let axis = face / 2u;
            let uv = face_cell(face, 0, 1, 1);
            var p0 = cell; p0[axis] += select(0, 1, (face & 1u) == 0u);
            let tangent_u = face_cell(face, 0, 1, 0); let tangent_v = face_cell(face, 0, 0, 1);
            let corners = array<vec3<i32>, 4>(p0, p0 + tangent_u, p0 + uv, p0 + tangent_v);
            let positive = face == 0u || face == 3u || face == 4u;
            for (var edge = 0u; edge < 4u; edge += 1u) {
                let a = corners[edge]; let b = corners[(edge + 1u) % 4u];
                // A patch boundary is on a tile face. Internal edge manifoldness
                // is independently checked by the vertex links below.
                var on_boundary = false;
                for (var d = 0u; d < 3u; d += 1u) {
                    on_boundary = on_boundary || (a[d] == b[d] && (a[d] == low[d] || a[d] == low[d] + size));
                }
                if (on_boundary && edge_count(descriptor, a, b, low, size) == 1u) {
                    if (boundary_count >= 192u) { return no_patch(); }
                    edges[boundary_count] = select(vec2(pack_point(b), pack_point(a)), vec2(pack_point(a), pack_point(b)), positive);
                    boundary_count += 1u;
                }
            }
            plane_counts[axis] += 1.0; plane_sums[axis] += f32(p0[axis]);
            mass += vec3<f32>(p0) + vec3<f32>(uv) * 0.5;
            direction += vec3<f32>(normal(face)); faces += 1u;
        }
    }}}
    if (boundary_count < 3u || faces == 0u) { return no_patch(); }
    for (var z = low.z; z <= low.z + size; z += 1) { for (var y = low.y; y <= low.y + size; y += 1) { for (var x = low.x; x <= low.x + size; x += 1) {
        if (!manifold_vertex(descriptor, vec3(x, y, z), low, size)) { return no_patch(); }
    }}}
    let start = edges[0].x; var current = start;
    for (var i = 0u; i < boundary_count; i += 1u) {
        contour[i] = unpack_point(current);
        var found = false;
        for (var j = 0u; j < boundary_count; j += 1u) {
            if (edges[j].x == current) {
                current = edges[j].y; edges[j].x = 0xffffffffu; found = true; break;
            }
        }
        if (!found || (current == start && i + 1u != boundary_count)) { return no_patch(); }
    }
    if (current != start) { return no_patch(); }
    // All projected edges must support the entire polygon. Unlike testing only
    // adjacent turns, this also rejects multiply-wound/self-crossing contours.
    for (var i = 0u; i < boundary_count; i += 1u) {
        let a = vec3<f32>(contour[i]); let edge = vec3<f32>(contour[(i + 1u) % boundary_count]) - a;
        for (var j = 0u; j < boundary_count; j += 1u) {
            if (dot(direction, cross(edge, vec3<f32>(contour[j]) - a)) < 0.0) { return no_patch(); }
        }
    }
    // Axis-aligned Hermite normals give a diagonal QEF. The mass point supplies
    // unconstrained axes; the candidate must project strictly inside the loop.
    var center = mass / f32(faces);
    for (var axis = 0u; axis < 3u; axis += 1u) { if (plane_counts[axis] > 0.0) { center[axis] = plane_sums[axis] / plane_counts[axis]; } }
    center = clamp(center, vec3<f32>(low), vec3<f32>(low + vec3(size)));
    for (var i = 0u; i < boundary_count; i += 1u) {
        let a = vec3<f32>(contour[i]); let edge = vec3<f32>(contour[(i + 1u) % boundary_count]) - a;
        if (dot(direction, cross(edge, center - a)) <= 0.0) { center = mass / f32(faces); break; }
    }
    for (var i = 0u; i < boundary_count; i += 1u) {
        let a = vec3<f32>(contour[i]); let edge = vec3<f32>(contour[(i + 1u) % boundary_count]) - a;
        if (dot(direction, cross(edge, center - a)) <= 0.0) { return no_patch(); }
    }
    // Both surfaces are disks with the SAME convex projected boundary and no
    // projection folds. At equal projection their separation along direction is
    // bounded by the full height slab, giving a conservative TWO-WAY bound.
    let unit = normalize(direction);
    var minimum = dot(unit, center); var maximum = minimum;
    for (var z = low.z; z < low.z + size; z += 1) { for (var y = low.y; y < low.y + size; y += 1) { for (var x = low.x; x < low.x + size; x += 1) {
        let cell = vec3(x, y, z);
        for (var face = 0u; face < 6u; face += 1u) {
            if (!visible(descriptor, face, cell)) { continue; }
            var a = cell; a[face / 2u] += select(0, 1, (face & 1u) == 0u);
            let u = face_cell(face, 0, 1, 0); let v = face_cell(face, 0, 0, 1);
            let corners = array<vec3<i32>, 4>(a, a + u, a + u + v, a + v);
            for (var i = 0u; i < 4u; i += 1u) { let h = dot(unit, vec3<f32>(corners[i])); minimum = min(minimum, h); maximum = max(maximum, h); }
        }
    }}}
    // Drop only EXACTLY collinear 3-D vertices; the boundary curve is unchanged.
    var count = boundary_count; var changed = true;
    while (changed && count > 3u) {
        changed = false;
        for (var i = 0u; i < count; i += 1u) {
            let a = contour[(i + count - 1u) % count]; let b = contour[i]; let c = contour[(i + 1u) % count];
            let turn = cross(vec3<f32>(b - a), vec3<f32>(c - b));
            if (all(turn == vec3(0.0)) && dot(vec3<f32>(b - a), vec3<f32>(c - b)) > 0.0) {
                for (var j = i; j + 1u < count; j += 1u) { contour[j] = contour[j + 1u]; }
                count -= 1u; changed = true; break;
            }
        }
    }
    // Float arithmetic guard, not a user quality threshold.
    let error = maximum - minimum + 32.0 * 1.1920929e-7 * 16.0;
    return Patch(count, error, center);
}

fn emit(slot: u32, lod: u32, request: Request, data: vec4<u32>) {
    let index = atomicAdd(&measurements[slot * 5u + lod].count, 1u);
    if (index < request.bounds.y) { quads[request.destination.z + index].data = data; }
}
fn greedy_plane(descriptor: ChunkDescriptor, slot: u32, lod: u32, low: vec3<i32>, size: i32, plane: u32, writing: bool, request: Request) -> u32 {
    let face = plane / u32(size); let slice = low[face / 2u] + i32(plane % u32(size));
    var u_axis = 0u; var v_axis = 1u;
    if (face < 2u) { u_axis = 1u; v_axis = 2u; } else if (face < 4u) { v_axis = 2u; }
    for (var v = 0; v < size; v += 1) {
        var row = 0u;
        for (var u = 0; u < size; u += 1) {
            if (visible(descriptor, face, face_cell(face, slice, low[u_axis] + u, low[v_axis] + v))) { row |= 1u << u32(u); }
        }
        rows[u32(v)] = row;
    }
    var count = 0u;
    for (var v = 0u; v < u32(size); v += 1u) {
        var u = 0u;
        while (u < u32(size)) {
            if ((rows[v] & (1u << u)) == 0u) { u += 1u; continue; }
            var width = 1u;
            while (u + width < u32(size) && (rows[v] & (1u << (u + width))) != 0u) { width += 1u; }
            let mask = ((1u << width) - 1u) << u;
            var height = 1u;
            while (v + height < u32(size) && (rows[v + height] & mask) == mask) { height += 1u; }
            for (var j = v; j < v + height; j += 1u) { rows[j] &= ~mask; }
            if (writing) { emit(slot, lod, request, vec4(face, u32(slice), (u32(low[u_axis]) + u) | ((u32(low[v_axis]) + v) << 8u), width | (height << 8u))); }
            count += 1u; u += width;
        }
    }
    return count;
}
fn greedy_box(descriptor: ChunkDescriptor, slot: u32, lod: u32, low: vec3<i32>, size: i32, writing: bool, request: Request) -> u32 {
    var count = 0u;
    for (var plane = 0u; plane < 6u * u32(size); plane += 1u) { count += greedy_plane(descriptor, slot, lod, low, size, plane, writing, request); }
    return count;
}
fn mesh_request(index: u32, group: u32, writing: bool) {
    if (index >= build_pass.descriptor_count) { return; }
    let request = requests[index]; let slot = request.destination.x;
    if (slot == 0xffffffffu) { return; }
    let descriptor = descriptors[slot];
    if (descriptor.metadata.x == 0u || descriptor.svo.z != build_pass.svo_segment) { return; }
    if (writing && request.destination.w != build_pass.geometry_segment) { return; }
    var lod = 0u; var tile = group;
    if (group >= 680u) { lod = 4u; tile -= 680u; }
    else if (group >= 672u) { lod = 3u; tile -= 672u; }
    else if (group >= 608u) { lod = 2u; tile -= 608u; }
    else if (group >= 96u) { lod = 1u; tile -= 96u; }
    if (writing && request.destination.y != lod) { return; }
    if (lod == 0u) {
        let count = greedy_plane(descriptor, slot, lod, vec3(0), 16, tile, writing, request);
        if (!writing) { atomicAdd(&measurements[slot * 5u].count, count); }
        return;
    }
    let size = 1u << lod; let grid = 16u / size;
    let low = vec3<i32>(vec3(tile % grid, (tile / grid) % grid, tile / (grid * grid)) * size);
    let exact = greedy_box(descriptor, slot, lod, low, i32(size), false, request);
    if (exact == 0u) { return; }
    var proxy = no_patch();
    // A disk fan needs at least three records. Exact tiles below that count
    // cannot benefit and need no topology/QEF work (notably superflat terrain).
    if (exact > 3u) { proxy = make_proxy(descriptor, low, i32(size)); }
    if (proxy.count > 0u && proxy.count < exact) {
        if (writing) {
            for (var i = 0u; i < proxy.count; i += 1u) {
                let a = pack_point(contour[i]); let b = pack_point(contour[(i + 1u) % proxy.count]);
                emit(slot, lod, request, vec4(0x80000000u | a | (b << 15u), bitcast<u32>(proxy.center.x), bitcast<u32>(proxy.center.y), bitcast<u32>(proxy.center.z)));
            }
        } else {
            atomicAdd(&measurements[slot * 5u + lod].count, proxy.count);
            atomicMax(&measurements[slot * 5u + lod].error_bits, bitcast<u32>(proxy.error));
        }
    } else {
        if (writing) { greedy_box(descriptor, slot, lod, low, i32(size), true, request); }
        else { atomicAdd(&measurements[slot * 5u + lod].count, exact); }
    }
}
@compute @workgroup_size(1)
fn prepare_boundary(@builtin(workgroup_id) group: vec3<u32>, @builtin(num_workgroups) size: vec3<u32>) {
    let index = group.y + group.z * size.y;
    if (index >= build_pass.descriptor_count) { return; }
    let slot = requests[index].destination.x;
    if (slot == 0xffffffffu) { return; }
    let descriptor = descriptors[slot];
    if (descriptor.svo.z != build_pass.svo_segment || descriptor.metadata.x == 0u) { return; }
    let face = group.x / 16u; let v = group.x % 16u;
    let slice = select(0, 15, (face & 1u) == 0u);
    var row = 0u;
    for (var u = 0u; u < 16u; u += 1u) { if (occupied(descriptor, face_cell(face, slice, i32(u), i32(v)))) { row |= 1u << u; } }
    boundary_rows[slot * 96u + group.x] = row;
}
@compute @workgroup_size(64)
fn reset_geometry(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= build_pass.descriptor_count) { return; }
    let request = requests[index]; let slot = request.destination.x;
    if (slot == 0xffffffffu || descriptors[slot].svo.z != build_pass.svo_segment) { return; }
    if (request.destination.y == 0xffffffffu) {
        for (var lod = 0u; lod < 5u; lod += 1u) {
            atomicStore(&measurements[slot * 5u + lod].count, 0u); atomicStore(&measurements[slot * 5u + lod].error_bits, 0u);
        }
    } else if (request.destination.w == build_pass.geometry_segment) { atomicStore(&measurements[slot * 5u + request.destination.y].count, 0u); }
}
@compute @workgroup_size(1)
fn count_models(@builtin(workgroup_id) group: vec3<u32>, @builtin(num_workgroups) size: vec3<u32>) { mesh_request(group.y + group.z * size.y, group.x, false); }
@compute @workgroup_size(1)
fn mesh_chunks(@builtin(workgroup_id) group: vec3<u32>, @builtin(num_workgroups) size: vec3<u32>) { mesh_request(group.y + group.z * size.y, group.x, true); }
@compute @workgroup_size(64)
fn gather_counts(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= build_pass.descriptor_count) { return; }
    let slot = requests[index].destination.x;
    if (slot == 0xffffffffu || descriptors[slot].svo.z != build_pass.svo_segment) { return; }
    let empty = atomicLoad(&measurements[slot * 5u].count) == 0u;
    var valid = true;
    for (var lod = 0u; lod < 5u; lod += 1u) {
        let count = atomicLoad(&measurements[slot * 5u + lod].count);
        let error = bitcast<f32>(atomicLoad(&measurements[slot * 5u + lod].error_bits));
        valid = valid && count <= 6u * 16u * 16u * 16u && error >= 0.0 && error <= bitcast<f32>(0x7f7fffffu) && (count == 0u) == empty;
        if (lod == 0u) { valid = valid && error == 0.0; }
    }
    if (!valid) {
        atomicOr(&pool.fault[0], 2u);
        atomicStore(&pool.fault[1], slot);
        return;
    }
    models[slot].generations.x = descriptors[slot].metadata.w;
    atomicAdd(&pool.activity[3], 1u);
}
fn release_page(slab: u32, page: u32) {
    let flag = 1u << (page % 32u);
    let previous = atomicAnd(&pool.slabs[slab].bitmap[page / 32u], ~flag);
    if ((previous & flag) == 0u) { atomicOr(&pool.fault[0], 8u); return; }
    if (atomicSub(&pool.slabs[slab].live_pages, 1u) == 0u) { atomicOr(&pool.fault[0], 8u); }
    atomicAdd(&pool.activity[1], 1u);
}
@compute @workgroup_size(64)
fn finalize_builds(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= build_pass.descriptor_count) { return; }
    let request = requests[index]; let slot = request.destination.x; let lod = request.destination.y;
    if (slot == 0xffffffffu || descriptors[slot].svo.z != build_pass.svo_segment || request.destination.w != build_pass.geometry_segment) { return; }
    let count = atomicLoad(&measurements[slot * 5u + lod].count);
    // Only successful publication retires the previous page. All prior-frame
    // reads precede this pass; allocation for this frame has already completed.
    if (count != request.bounds.x || count > request.bounds.y) {
        atomicOr(&pool.fault[0], 4u);
        atomicStore(&pool.fault[1], slot);
    }
    if (atomicLoad(&pool.fault[0]) != 0u) {
        if (request.bounds.z != 0xffffffffu) { release_page(request.bounds.z, request.bounds.w); }
        requests[index].destination.x = 0xffffffffu;
        return;
    }
    let previous = models[slot];
    for (var other = 0u; other < 5u; other += 1u) { descriptors[slot].draws[other] = vec4(0u); }
    if (count != 0u) { descriptors[slot].draws[lod] = vec4(count * 6u, request.destination.z * 6u, request.destination.w, 1u); }
    descriptors[slot].metadata.z = lod;
    models[slot].generations.y = descriptors[slot].metadata.w;
    models[slot].generations.z = lod;
    models[slot].generations.w = u32(count != 0u);
    models[slot].allocation = vec4(request.bounds.z, request.bounds.w, 0u, descriptors[slot].svo.w);
    atomicAdd(&pool.activity[2], 1u);
    if (previous.generations.w != 0u) { release_page(previous.allocation.x, previous.allocation.y); }
    requests[index].destination.x = 0xffffffffu;
}
