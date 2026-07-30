//! Procedural generation algorithms independent of ECS and storage crates.

pub mod infinite_spheres;
pub mod maze;

/// One material sample emitted by a procedural generator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedVoxel {
    pub position: [i32; 3],
    pub material_id: u16,
}
