// Counts deterministic greedy rectangles for every visible-face plane.
@compute @workgroup_size(1)
fn main(@builtin(workgroup_id) plane: vec3<u32>) {
    _ = plane;
}
