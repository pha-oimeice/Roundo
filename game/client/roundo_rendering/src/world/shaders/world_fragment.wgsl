struct FragmentInput {
    @location(0) local_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) @interpolate(flat) lod: u32,
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

@fragment
fn fragment(input: FragmentInput) -> @location(0) vec4<f32> {
    let owned_position = input.local_position - face_normal(input.face) * 0.001;
    let cell_size = i32(1u << input.lod);
    let lod_cell = vec3<i32>(floor(owned_position / f32(cell_size)));
    let local_cell = lod_cell * cell_size;
    let cell = input.chunk_coordinate * 16 + local_cell;
    let base_color = procedural_color(cell, input.face, input.material_seed);
    let contrast = exp2(-f32(input.lod));
    let color = vec3(0.5) + (base_color - vec3(0.5)) * contrast;
    return vec4(color, 1.0);
}
