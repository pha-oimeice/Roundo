#import bevy_render::view::View

struct GeneratedVertex {
    position_and_face: vec4<f32>,
}

struct ChunkUniform {
    world_from_chunk: mat4x4<f32>,
    svo: vec4<u32>,
    chunk_coordinate: vec4<i32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<storage, read> vertices: array<GeneratedVertex>;
@group(1) @binding(1) var<uniform> chunk: ChunkUniform;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) @interpolate(flat) face: u32,
    @location(3) @interpolate(flat) lod: u32,
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

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let generated = vertices[vertex_index];
    let local_position = generated.position_and_face.xyz;
    let face_and_lod = u32(generated.position_and_face.w);
    let face = face_and_lod & 7u;
    let lod = face_and_lod >> 3u;
    let world_position = chunk.world_from_chunk * vec4(local_position, 1.0);
    let world_normal = normalize((chunk.world_from_chunk * vec4(face_normal(face), 0.0)).xyz);

    var output: VertexOutput;
    output.clip_position = view.clip_from_world * world_position;
    output.local_position = local_position;
    output.world_normal = world_normal;
    output.face = face;
    output.lod = lod;
    return output;
}
