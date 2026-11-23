//! Deterministic, neighbor-independent chunk generation for an infinite world.

use super::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkGenerator, EMPTY_MATERIAL_ID, GeneratedChunk,
    SOLID_MATERIAL_ID, chunk_generator::voxel_index,
};
use std::f64::consts::TAU;

/// Multiplier applied to each cell's deterministic sphere-radius sample.
pub const DEFAULT_SPHERE_RADIUS_SCALE: f64 = 10.0;

const SPHERE_CELL_EDGE_LENGTH: i64 = 64;
const MAXIMUM_SAMPLED_NORMAL_MAGNITUDE: f64 = 8.7;
const MAXIMUM_SPHERE_RADIUS: f64 =
    (MAXIMUM_SAMPLED_NORMAL_MAGNITUDE + 1.0) * DEFAULT_SPHERE_RADIUS_SCALE;

/// Pure, order-independent mapping from chunk coordinate to generated voxels.
///
/// One deterministic sphere is assigned to every 64-unit spatial cell. Callers
/// should keep `radius_scale` finite and nonnegative; the public field does not
/// enforce validity, and very large values can make generation impractically
/// expensive by expanding the sampled cell range.
#[derive(Clone, Copy, Debug)]
pub struct InfiniteSphereGenerator {
    /// Seed mixed independently with each sphere-cell coordinate.
    pub seed: u64,
    /// Nonnegative radius multiplier; generation clamps negative values to zero.
    pub radius_scale: f64,
}

impl InfiniteSphereGenerator {
    /// Creates a generator with [`DEFAULT_SPHERE_RADIUS_SCALE`].
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            radius_scale: DEFAULT_SPHERE_RADIUS_SCALE,
        }
    }

    fn sphere_in_cell(&self, cell: [i64; 3]) -> Sphere {
        let mut random = DeterministicRandom::new(hash_cell(self.seed, cell));
        let cell_origin = cell.map(|value| value.saturating_mul(SPHERE_CELL_EDGE_LENGTH) as f64);
        let center = std::array::from_fn(|axis| {
            cell_origin[axis] + random.next_open_unit() * SPHERE_CELL_EDGE_LENGTH as f64
        });
        let standard_normal = box_muller(&mut random);
        let radius = (standard_normal.abs() + 1.0) * self.radius_scale.max(0.0);
        Sphere { center, radius }
    }
}

impl ChunkGenerator for InfiniteSphereGenerator {
    /// Generates any chunk without reading or generating adjacent chunks.
    ///
    /// Equal generator fields and coordinates produce the same voxel sequence
    /// regardless of prior calls. Coordinate multiplication saturates at
    /// `i64` bounds before conversion to generation space.
    fn generate_chunk(&self, coordinate: ChunkCoordinate) -> GeneratedChunk {
        let chunk_min =
            coordinate.map(|value| value.saturating_mul(CHUNK_EDGE_LENGTH as i64) as f64);
        let chunk_max = chunk_min.map(|value| value + CHUNK_EDGE_LENGTH as f64);
        let maximum_radius = (MAXIMUM_SAMPLED_NORMAL_MAGNITUDE + 1.0) * self.radius_scale.max(0.0);
        let minimum_cell: [i64; 3] = std::array::from_fn(|axis| {
            ((chunk_min[axis] - maximum_radius) / SPHERE_CELL_EDGE_LENGTH as f64).floor() as i64
        });
        let maximum_cell: [i64; 3] = std::array::from_fn(|axis| {
            ((chunk_max[axis] + maximum_radius) / SPHERE_CELL_EDGE_LENGTH as f64).floor() as i64
        });
        let mut voxels = vec![EMPTY_MATERIAL_ID; CHUNK_EDGE_LENGTH.pow(3)];

        for cell_x in minimum_cell[0]..=maximum_cell[0] {
            for cell_y in minimum_cell[1]..=maximum_cell[1] {
                for cell_z in minimum_cell[2]..=maximum_cell[2] {
                    let sphere = self.sphere_in_cell([cell_x, cell_y, cell_z]);
                    if sphere_intersects_box(sphere, chunk_min, chunk_max) {
                        rasterize_sphere(&mut voxels, chunk_min, sphere);
                    }
                }
            }
        }

        GeneratedChunk::from_voxels(voxels)
            .expect("the sphere generator always emits one complete chunk")
    }
}

impl Default for InfiniteSphereGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

#[derive(Clone, Copy)]
struct Sphere {
    center: [f64; 3],
    radius: f64,
}

fn rasterize_sphere(voxels: &mut [u16], chunk_min: [f64; 3], sphere: Sphere) {
    let local_min: [usize; 3] = std::array::from_fn(|axis| {
        ((sphere.center[axis] - sphere.radius - chunk_min[axis]).floor() as i64)
            .clamp(0, CHUNK_EDGE_LENGTH as i64 - 1) as usize
    });
    let local_max: [usize; 3] = std::array::from_fn(|axis| {
        ((sphere.center[axis] + sphere.radius - chunk_min[axis]).ceil() as i64)
            .clamp(0, CHUNK_EDGE_LENGTH as i64 - 1) as usize
    });
    let radius_squared = sphere.radius * sphere.radius;

    for z in local_min[2]..=local_max[2] {
        for y in local_min[1]..=local_max[1] {
            for x in local_min[0]..=local_max[0] {
                let voxel_center = [
                    chunk_min[0] + x as f64 + 0.5,
                    chunk_min[1] + y as f64 + 0.5,
                    chunk_min[2] + z as f64 + 0.5,
                ];
                let distance_squared = (0..3)
                    .map(|axis| (voxel_center[axis] - sphere.center[axis]).powi(2))
                    .sum::<f64>();
                if distance_squared <= radius_squared {
                    voxels[voxel_index([x, y, z])] = SOLID_MATERIAL_ID;
                }
            }
        }
    }
}

fn sphere_intersects_box(sphere: Sphere, minimum: [f64; 3], maximum: [f64; 3]) -> bool {
    let distance_squared = (0..3)
        .map(|axis| {
            if sphere.center[axis] < minimum[axis] {
                (minimum[axis] - sphere.center[axis]).powi(2)
            } else if sphere.center[axis] > maximum[axis] {
                (sphere.center[axis] - maximum[axis]).powi(2)
            } else {
                0.0
            }
        })
        .sum::<f64>();
    distance_squared <= sphere.radius * sphere.radius
}

fn box_muller(random: &mut DeterministicRandom) -> f64 {
    (-2.0 * random.next_open_unit().ln()).sqrt() * (TAU * random.next_open_unit()).cos()
}

fn hash_cell(seed: u64, coordinate: [i64; 3]) -> u64 {
    coordinate
        .into_iter()
        .enumerate()
        .fold(seed ^ 0xa076_1d64_78bd_642f, |hash, (axis, value)| {
            mix64(hash ^ (value as u64).wrapping_mul(COORDINATE_SALTS[axis]))
        })
}

const COORDINATE_SALTS: [u64; 3] = [
    0xe703_7ed1_a0b4_28db,
    0x8ebc_6af0_9c88_c6e3,
    0x5899_65cc_7537_4cc3,
];

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 32;
    value = value.wrapping_mul(0xd6e8_feb8_6659_fd93);
    value ^= value >> 32;
    value = value.wrapping_mul(0xd6e8_feb8_6659_fd93);
    value ^ (value >> 32)
}

struct DeterministicRandom {
    state: u64,
}

impl DeterministicRandom {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix64(self.state)
    }

    fn next_open_unit(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / ((1_u64 << 53) as f64);
        ((self.next_u64() >> 11) as f64 + 0.5) * SCALE
    }
}

const _: () = assert!(MAXIMUM_SPHERE_RADIUS < 128.0);
