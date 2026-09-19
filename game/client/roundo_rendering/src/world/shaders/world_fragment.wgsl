const MATERIAL_FOOTPRINT_LOD_BIAS: f32 = 1.25;

struct FragmentInput {
    @location(0) local_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) @interpolate(flat) geometry_lod: u32,
    @location(4) @interpolate(flat) chunk_coordinate: vec3<i32>,
    @location(5) @interpolate(flat) material_seed: u32,
}

fn hash_u32(value: u32) -> u32 {
    var hash = value;
    hash = (hash ^ (hash >> 16u)) * 0x7feb352du;
    hash = (hash ^ (hash >> 15u)) * 0x846ca68bu;
    return hash ^ (hash >> 16u);
}

fn face_normal(face: u32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3(1.0, 0.0, 0.0); }
        case 1u: { return vec3(-1.0, 0.0, 0.0); }
        case 2u: { return vec3(0.0, 1.0, 0.0); }
        case 3u: { return vec3(0.0, -1.0, 0.0); }
        case 4u: { return vec3(0.0, 0.0, 1.0); }
        default: { return vec3(0.0, 0.0, -1.0); }
    }
}

fn procedural_color(cell: vec3<i32>, face: u32, seed: u32) -> vec3<f32> {
    var hash = hash_u32(bitcast<u32>(cell.x) ^ seed);
    hash = hash_u32(hash ^ bitcast<u32>(cell.y));
    hash = hash_u32(hash ^ bitcast<u32>(cell.z));
    hash = hash_u32(hash ^ face);
    return vec3<f32>(
        f32(hash & 0xffu),
        f32((hash >> 8u) & 0xffu),
        f32((hash >> 16u) & 0xffu),
    ) / 255.0;
}

fn lod_color(cell: vec3<i32>, cell_size: i32, face: u32, seed: u32) -> vec3<f32> {
    return procedural_color(cell * cell_size, face, seed);
}

fn interpolated_lod_color(
    local_position: vec3<f32>,
    chunk_coordinate: vec3<i32>,
    cell_size: i32,
    face: u32,
    seed: u32,
) -> vec3<f32> {
    // Hash samples live at coarse-Cell centers. Keep fractional arithmetic
    // Chunk-local so distant integer coordinates do not erase interpolation
    // precision when converted to f32.
    let cells_per_chunk = 16 / cell_size;
    let chunk_cell_origin = chunk_coordinate * cells_per_chunk;
    let local_sample_position = local_position / f32(cell_size) - vec3(0.5);
    let base = chunk_cell_origin + vec3<i32>(floor(local_sample_position));
    let fraction = fract(local_sample_position);
    let smoothed = fraction * fraction * (vec3(3.0) - 2.0 * fraction);
    let owned_cell = chunk_cell_origin + vec3<i32>(floor(local_position / f32(cell_size)));
    var tangent_u: vec3<i32>;
    var tangent_v: vec3<i32>;
    var u: f32;
    var v: f32;
    if (face < 2u) {
        tangent_u = vec3(0, 1, 0);
        tangent_v = vec3(0, 0, 1);
        u = smoothed.y;
        v = smoothed.z;
    } else if (face < 4u) {
        tangent_u = vec3(1, 0, 0);
        tangent_v = vec3(0, 0, 1);
        u = smoothed.x;
        v = smoothed.z;
    } else {
        tangent_u = vec3(1, 0, 0);
        tangent_v = vec3(0, 1, 0);
        u = smoothed.x;
        v = smoothed.y;
    }
    // Preserve the owned normal layer while selecting the two tangent lattice
    // coordinates on either side of the fragment.
    var c00 = base;
    if (face < 2u) { c00.x = owned_cell.x; }
    else if (face < 4u) { c00.y = owned_cell.y; }
    else { c00.z = owned_cell.z; }
    let c10 = c00 + tangent_u;
    let c01 = c00 + tangent_v;
    let c11 = c00 + tangent_u + tangent_v;
    let row0 = mix(
        lod_color(c00, cell_size, face, seed),
        lod_color(c10, cell_size, face, seed),
        u,
    );
    let row1 = mix(
        lod_color(c01, cell_size, face, seed),
        lod_color(c11, cell_size, face, seed),
        u,
    );
    return mix(row0, row1, v);
}

fn material_level_color(
    local_position: vec3<f32>,
    chunk_coordinate: vec3<i32>,
    level: u32,
    face: u32,
    seed: u32,
) -> vec3<f32> {
    let cell_size = i32(1u << level);
    let chunk_cell_origin = chunk_coordinate * (16 / cell_size);
    let owned_cell = chunk_cell_origin + vec3<i32>(floor(local_position / f32(cell_size)));
    let sharp_color = lod_color(owned_cell, cell_size, face, seed);
    let smooth_color = interpolated_lod_color(
        local_position,
        chunk_coordinate,
        cell_size,
        face,
        seed,
    );
    let interpolation = 1.0 - exp2(-f32(level));
    let filtered = mix(sharp_color, smooth_color, interpolation);
    let contrast = exp2(-0.5 * f32(level));
    return vec3(0.5) + (filtered - vec3(0.5)) * contrast;
}

fn material_footprint_lod(local_position: vec3<f32>, face: u32) -> f32 {
    let dx = dpdx(local_position);
    let dy = dpdy(local_position);
    var dx_tangent: vec2<f32>;
    var dy_tangent: vec2<f32>;
    if (face < 2u) {
        dx_tangent = dx.yz;
        dy_tangent = dy.yz;
    } else if (face < 4u) {
        dx_tangent = dx.xz;
        dy_tangent = dy.xz;
    } else {
        dx_tangent = dx.xy;
        dy_tangent = dy.xy;
    }
    let footprint = max(length(dx_tangent), length(dy_tangent));
    // Enter the filtered material levels slightly earlier than the strict
    // one-Cell-per-pixel boundary to suppress distant temporal replacement.
    let biased_footprint = footprint * MATERIAL_FOOTPRINT_LOD_BIAS;
    return clamp(log2(max(biased_footprint, 1.0)), 0.0, 4.0);
}

@fragment
fn fragment(input: FragmentInput) -> @location(0) vec4<f32> {
    let owned_position = input.local_position - face_normal(input.face) * 0.001;
    // Material detail is selected from the actual fragment footprint. The
    // geometry tier carried by the vertex is intentionally not consulted.
    let material_lod = material_footprint_lod(input.local_position, input.face);
    let lower_level = u32(floor(material_lod));
    let upper_level = min(lower_level + 1u, 4u);
    let lower_color = material_level_color(
        owned_position,
        input.chunk_coordinate,
        lower_level,
        input.face,
        input.material_seed,
    );
    let upper_color = material_level_color(
        owned_position,
        input.chunk_coordinate,
        upper_level,
        input.face,
        input.material_seed,
    );
    return vec4(mix(lower_color, upper_color, fract(material_lod)), 1.0);
}
