// Builds orientation/slice visible-face bit masks from selected SVO cells.
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    _ = invocation;
}
