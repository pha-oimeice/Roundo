struct PackedSvoNode {
    first_child: u32,
    child_mask: u32,
    data: u32,
    reserved: u32,
}

struct GpuChunkDescriptor {
    svo_first_node: u32,
    svo_node_count: u32,
    root_index: u32,
    maximum_depth: u32,
    chunk_x: i32,
    chunk_y: i32,
    chunk_z: i32,
    generation: u32,
}

const NO_CHILDREN: u32 = 0xffffffffu;
const EMPTY_VOXEL_ID: u32 = 0u;
