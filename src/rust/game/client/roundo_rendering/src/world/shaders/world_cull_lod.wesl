import bevy_render::view::View;

struct ChunkDescriptor {
    world_from_chunk: mat4x4<f32>,
    chunk_coordinate: vec4<i32>,
    // Per LOD: vertex count, first vertex, geometry segment, allocated flag.
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
@group(1) @binding(0) var<storage, read> descriptors: array<ChunkDescriptor>;
@group(1) @binding(1) var<storage, read_write> visible_draws: array<DrawIndirect>;
@group(1) @binding(2) var<storage, read_write> segment_counts: array<atomic<u32>>;

@group(1) @binding(3) var<uniform> layer: vec4<u32>;
const MAX_SEGMENTS: u32 = 16u;

fn chunk_visible(world_from_chunk: mat4x4<f32>) -> bool {
    var outside = array<bool, 6>(true, true, true, true, true, true);
    for (var z = 0u; z < 2u; z += 1u) {
        for (var y = 0u; y < 2u; y += 1u) {
            for (var x = 0u; x < 2u; x += 1u) {
                let local = vec4<f32>(f32(x) * 16.0, f32(y) * 16.0, f32(z) * 16.0, 1.0);
                let clip = view.clip_from_world * world_from_chunk * local;
                // Keep a small homogeneous guard band so camera motion cannot
                // toggle a far Chunk solely from f32 clip-plane roundoff.
                let margin = max(abs(clip.w) * 1.0e-4, 1.0e-4);
                outside[0] = outside[0] && clip.x < -clip.w - margin;
                outside[1] = outside[1] && clip.x > clip.w + margin;
                outside[2] = outside[2] && clip.y < -clip.w - margin;
                outside[3] = outside[3] && clip.y > clip.w + margin;
                outside[4] = outside[4] && clip.z < -margin;
                outside[5] = outside[5] && clip.z > clip.w + margin;
            }
        }
    }
    return !(outside[0] || outside[1] || outside[2] || outside[3] || outside[4] || outside[5]);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= arrayLength(&descriptors)) { return; }
    let descriptor = descriptors[index];
    if (descriptor.metadata.z != layer.x) { return; }
    let draw = descriptor.draws[layer.x];
    let segment = draw.z;
    if (descriptor.metadata.x == 0u || draw.x == 0u || segment >= MAX_SEGMENTS) { return; }
    if (!chunk_visible(descriptor.world_from_chunk)) { return; }
    let output_index = atomicAdd(&segment_counts[segment], 1u);
    if (output_index >= layer.y) { return; }
    visible_draws[segment * layer.y + output_index] = DrawIndirect(
        draw.x,
        1u,
        draw.y,
        index,
    );
}
