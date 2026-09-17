#import bevy_render::view::View

struct GeneratedQuad {
    data: vec4<u32>,
}

struct RenderChunkDescriptor {
    world_from_chunk: mat4x4<f32>,
    chunk_coordinate: vec4<i32>,
    draws: array<vec4<u32>, 5>,
    // x = active, y = material seed, z = selected LOD, w = source generation
    metadata: vec4<u32>,
    svo: vec4<u32>,
    neighbor_slots: array<vec4<u32>, 2>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<storage, read> quads: array<GeneratedQuad>;
@group(1) @binding(1) var<storage, read> chunks: array<RenderChunkDescriptor>;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) @interpolate(flat) lod: u32,
    @location(4) @interpolate(flat) chunk_coordinate: vec3<i32>,
    @location(5) @interpolate(flat) material_seed: u32,
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

fn face_corner(face: u32, slice: u32, u: f32, v: f32, lod: u32) -> vec3<f32> {
    let s = f32(slice);
    var corner: vec3<f32>;
    switch face {
        case 0u: { corner = vec3(s + 1.0, u, v); }
        case 1u: { corner = vec3(s, u, v); }
        case 2u: { corner = vec3(u, s + 1.0, v); }
        case 3u: { corner = vec3(u, s, v); }
        case 4u: { corner = vec3(u, v, s + 1.0); }
        default: { corner = vec3(u, v, s); }
    }
    return corner * f32(1u << lod);
}

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) chunk_index: u32,
) -> VertexOutput {
    let generated = quads[vertex_index / 6u].data;
    let corner_index = vertex_index % 6u;
    let chunk = chunks[chunk_index];
    let face = generated.x;
    let slice = generated.y;
    let u = generated.z & 255u;
    let v = generated.z >> 8u;
    let width = generated.w & 255u;
    let height = (generated.w >> 8u) & 255u;
    let lod = generated.w >> 16u;
    let corners = array<vec2<f32>, 4>(
        vec2(f32(u), f32(v)),
        vec2(f32(u + width), f32(v)),
        vec2(f32(u + width), f32(v + height)),
        vec2(f32(u), f32(v + height)),
    );
    let positive_winding = face == 0u || face == 3u || face == 4u;
    let positive = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let negative = array<u32, 6>(0u, 2u, 1u, 0u, 3u, 2u);
    let selected_corner = select(negative[corner_index], positive[corner_index], positive_winding);
    let uv = corners[selected_corner];
    let local_position = face_corner(face, slice, uv.x, uv.y, lod);
    let world_position = chunk.world_from_chunk * vec4(local_position, 1.0);
    let world_normal = normalize((chunk.world_from_chunk * vec4(face_normal(face), 0.0)).xyz);

    var output: VertexOutput;
    output.clip_position = view.clip_from_world * world_position;
    output.local_position = local_position;
    output.world_normal = world_normal;
    output.face = face;
    output.lod = lod;
    output.chunk_coordinate = chunk.chunk_coordinate.xyz;
    output.material_seed = chunk.metadata.y;
    return output;
}
