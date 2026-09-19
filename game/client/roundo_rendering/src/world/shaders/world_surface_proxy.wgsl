// Hierarchical QEF reduction for adaptive-dual-contouring Surface Proxies.
// Leaf Hermite extraction and topology validation will populate the first
// level; this pass is deliberately independent of world axes and reduces every
// request and parent node in parallel.

struct QefSummary {
    // xx, xy, xz, yy
    ata0: vec4<f32>,
    // yz, zz, atb.x, atb.y
    ata1: vec4<f32>,
    // atb.z, btb, mass_sum.x, mass_sum.y
    qef_data: vec4<f32>,
    // mass_sum.z, sample_count, conservative_error, topology_flags(bitcast)
    proxy_data: vec4<f32>,
}

struct ReducePass {
    source_offset: u32,
    target_offset: u32,
    source_edge: u32,
    target_count: u32,
    summaries_per_request: u32,
    request_count: u32,
    _reserved0: u32,
    _reserved1: u32,
}

@group(0) @binding(0) var<storage, read_write> summaries: array<QefSummary>;
@group(0) @binding(1) var<uniform> reduce_pass: ReducePass;

fn zero_summary() -> QefSummary {
    return QefSummary(vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0));
}

fn merged(left: QefSummary, right: QefSummary) -> QefSummary {
    var result: QefSummary;
    result.ata0 = left.ata0 + right.ata0;
    result.ata1 = left.ata1 + right.ata1;
    result.qef_data = left.qef_data + right.qef_data;
    result.proxy_data.x = left.proxy_data.x + right.proxy_data.x;
    result.proxy_data.y = left.proxy_data.y + right.proxy_data.y;
    result.proxy_data.z = max(left.proxy_data.z, right.proxy_data.z);
    let left_flags = bitcast<u32>(left.proxy_data.w);
    let right_flags = bitcast<u32>(right.proxy_data.w);
    result.proxy_data.w = bitcast<f32>(left_flags | right_flags);
    return result;
}

fn flatten_cell(cell: vec3<u32>, edge: u32) -> u32 {
    return cell.x + cell.y * edge + cell.z * edge * edge;
}

@compute @workgroup_size(64)
fn reduce_qef_depth(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let target_index = invocation.x;
    let request_index = invocation.y;
    if (target_index >= reduce_pass.target_count || request_index >= reduce_pass.request_count) {
        return;
    }
    let target_edge = reduce_pass.source_edge / 2u;
    let target_xy = target_edge * target_edge;
    let target_cell = vec3(
        target_index % target_edge,
        (target_index / target_edge) % target_edge,
        target_index / target_xy,
    );
    let request_base = request_index * reduce_pass.summaries_per_request;
    var parent = zero_summary();
    for (var octant = 0u; octant < 8u; octant += 1u) {
        let child_offset = vec3(
            octant & 1u,
            (octant >> 1u) & 1u,
            (octant >> 2u) & 1u,
        );
        let child_cell = target_cell * 2u + child_offset;
        let child_index = request_base
            + reduce_pass.source_offset
            + flatten_cell(child_cell, reduce_pass.source_edge);
        parent = merged(parent, summaries[child_index]);
    }
    summaries[request_base + reduce_pass.target_offset + target_index] = parent;
}
