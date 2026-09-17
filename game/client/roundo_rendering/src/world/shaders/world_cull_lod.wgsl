#import bevy_render::view::View

struct ChunkDescriptor {
    world_from_chunk: mat4x4<f32>,
    chunk_coordinate: vec4<i32>,
    // Per LOD: vertex count, first vertex, geometry segment, reserved.
    draws: array<vec4<u32>, 5>,
    // active, material seed, previous selected LOD, source generation
    metadata: vec4<u32>,
    svo: vec4<u32>,
    neighbor_slots: array<vec4<u32>, 2>,
}

struct DrawIndirect {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<storage, read_write> descriptors: array<ChunkDescriptor>;
@group(1) @binding(1) var<storage, read_write> visible_draws: array<DrawIndirect>;
@group(1) @binding(2) var<storage, read_write> segment_counts: array<atomic<u32>>;

const MAX_CHUNKS: u32 = 262144u;
const MAX_SEGMENTS: u32 = 16u;

fn selected_lod(world_from_chunk: mat4x4<f32>, committed: u32) -> u32 {
    let axis_x = world_from_chunk[0].xyz;
    let axis_y = world_from_chunk[1].xyz;
    let axis_z = world_from_chunk[2].xyz;
    let from_origin = view.world_position - world_from_chunk[3].xyz;
    let local_camera = vec3(
        dot(from_origin, axis_x) / dot(axis_x, axis_x),
        dot(from_origin, axis_y) / dot(axis_y, axis_y),
        dot(from_origin, axis_z) / dot(axis_z, axis_z),
    );
    let camera_from_target = floor(local_camera / 16.0);
    let axis_scale = vec3(length(axis_x), length(axis_y), length(axis_z));
    let physical_delta = camera_from_target * 16.0 * axis_scale;
    let distance_squared = dot(physical_delta, physical_delta);
    let effective_scale = max(axis_scale.x, max(axis_scale.y, axis_scale.z));
    let focal_pixels = max(view.viewport.w, 1.0) * abs(view.clip_from_view[1][1]) * 0.5;
    let coarsening = 1.125 * 1.125;
    var lod = min(committed, 4u);
    while (lod < 4u) {
        let next_cell = f32(1u << (lod + 1u)) * effective_scale;
        let threshold = next_cell * focal_pixels / 2.0;
        if (distance_squared < threshold * threshold * coarsening) { break; }
        lod += 1u;
    }
    while (lod > 0u) {
        let cell = f32(1u << lod) * effective_scale;
        let threshold = cell * focal_pixels / 2.0;
        if (distance_squared >= threshold * threshold) { break; }
        lod -= 1u;
    }
    return lod;
}

fn chunk_visible(world_from_chunk: mat4x4<f32>) -> bool {
    var outside = array<bool, 6>(true, true, true, true, true, true);
    for (var z = 0u; z < 2u; z += 1u) {
        for (var y = 0u; y < 2u; y += 1u) {
            for (var x = 0u; x < 2u; x += 1u) {
                let local = vec4<f32>(f32(x) * 16.0, f32(y) * 16.0, f32(z) * 16.0, 1.0);
                let clip = view.clip_from_world * world_from_chunk * local;
                outside[0] = outside[0] && clip.x < -clip.w;
                outside[1] = outside[1] && clip.x > clip.w;
                outside[2] = outside[2] && clip.y < -clip.w;
                outside[3] = outside[3] && clip.y > clip.w;
                outside[4] = outside[4] && clip.z < 0.0;
                outside[5] = outside[5] && clip.z > clip.w;
            }
        }
    }
    return !(outside[0] || outside[1] || outside[2] || outside[3] || outside[4] || outside[5]);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= arrayLength(&descriptors)) { return; }
    var descriptor = descriptors[index];
    let lod = selected_lod(descriptor.world_from_chunk, descriptor.metadata.z);
    descriptor.metadata.z = lod;
    descriptors[index] = descriptor;
    let draw = descriptor.draws[lod];
    let segment = draw.z;
    if (descriptor.metadata.x == 0u || draw.x == 0u || segment >= MAX_SEGMENTS) { return; }
    if (!chunk_visible(descriptor.world_from_chunk)) { return; }
    let output_index = atomicAdd(&segment_counts[segment], 1u);
    if (output_index >= MAX_CHUNKS) { return; }
    visible_draws[segment * MAX_CHUNKS + output_index] = DrawIndirect(
        draw.x,
        1u,
        draw.y,
        index,
    );
}
