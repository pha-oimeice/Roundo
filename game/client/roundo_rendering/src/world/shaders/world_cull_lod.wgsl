// GPU visibility and projected-size SVO-level selection entry point.
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    _ = invocation;
}
