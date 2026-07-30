//! Deterministic, neighbor-independent chunk generation for an infinite world.

use std::{f64::consts::TAU, sync::Arc};

pub const CHUNK_EDGE_LENGTH: usize = 16;
pub const EMPTY_MATERIAL_ID: u16 = 0;
pub const SOLID_MATERIAL_ID: u16 = 1;
pub const DEFAULT_SPHERE_RADIUS_SCALE: f64 = 10.0;

const SPHERE_CELL_EDGE_LENGTH: i64 = 64;
const MAXIMUM_SAMPLED_NORMAL_MAGNITUDE: f64 = 8.7;
const MAXIMUM_SPHERE_RADIUS: f64 =
    (MAXIMUM_SAMPLED_NORMAL_MAGNITUDE + 1.0) * DEFAULT_SPHERE_RADIUS_SCALE;

/// Dense algorithm output for one fixed-size chunk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedChunk {
    pub coordinate: [i64; 3],
    voxels: Arc<[u16]>,
}

impl GeneratedChunk {
    pub fn empty(coordinate: [i64; 3]) -> Self {
        Self {
            coordinate,
            voxels: vec![EMPTY_MATERIAL_ID; CHUNK_EDGE_LENGTH.pow(3)].into(),
        }
    }

    pub fn voxel(&self, local_position: [usize; 3]) -> Option<u16> {
        voxel_index(local_position).map(|index| self.voxels[index])
    }

    pub fn from_voxels(coordinate: [i64; 3], voxels: Vec<u16>) -> Option<Self> {
        (voxels.len() == CHUNK_EDGE_LENGTH.pow(3)).then(|| Self {
            coordinate,
            voxels: voxels.into(),
        })
    }

    pub fn voxels(&self) -> &[u16] {
        &self.voxels
    }

    pub fn into_voxels(self) -> Vec<u16> {
        self.voxels.as_ref().to_vec()
    }

    pub fn is_empty(&self) -> bool {
        self.voxels
            .iter()
            .all(|material| *material == EMPTY_MATERIAL_ID)
    }
}

/// A pure function object implementing `f(x, y, z) -> GeneratedChunk`.
#[derive(Clone, Copy, Debug)]
pub struct InfiniteSphereGenerator {
    pub seed: u64,
    pub radius_scale: f64,
}

impl InfiniteSphereGenerator {
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            radius_scale: DEFAULT_SPHERE_RADIUS_SCALE,
        }
    }

    /// Generates any chunk without reading or generating adjacent chunks.
    pub fn generate_chunk(&self, x: i64, y: i64, z: i64) -> GeneratedChunk {
        let coordinate = [x, y, z];
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
        let mut chunk = GeneratedChunk::empty(coordinate);

        for cell_x in minimum_cell[0]..=maximum_cell[0] {
            for cell_y in minimum_cell[1]..=maximum_cell[1] {
                for cell_z in minimum_cell[2]..=maximum_cell[2] {
                    let sphere = self.sphere_in_cell([cell_x, cell_y, cell_z]);
                    if sphere_intersects_box(sphere, chunk_min, chunk_max) {
                        rasterize_sphere(Arc::make_mut(&mut chunk.voxels), chunk_min, sphere);
                    }
                }
            }
        }

        chunk
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

impl Default for InfiniteSphereGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Generates one chunk with the default seed and radius scale.
pub fn generate_chunk(x: i64, y: i64, z: i64) -> GeneratedChunk {
    InfiniteSphereGenerator::default().generate_chunk(x, y, z)
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
                    voxels[voxel_index([x, y, z]).expect("bounded local position")] =
                        SOLID_MATERIAL_ID;
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

fn voxel_index([x, y, z]: [usize; 3]) -> Option<usize> {
    if x >= CHUNK_EDGE_LENGTH || y >= CHUNK_EDGE_LENGTH || z >= CHUNK_EDGE_LENGTH {
        return None;
    }
    Some(x + y * CHUNK_EDGE_LENGTH + z * CHUNK_EDGE_LENGTH * CHUNK_EDGE_LENGTH)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_generation_is_order_independent() {
        let generator = InfiniteSphereGenerator::new(7);
        let first = generator.generate_chunk(-3, 4, 9);
        let _unrelated = generator.generate_chunk(100, -200, 300);
        let second = generator.generate_chunk(-3, 4, 9);
        assert_eq!(first, second);
    }

    #[test]
    fn supports_negative_chunk_coordinates() {
        let chunk = InfiniteSphereGenerator::default().generate_chunk(-1, -1, -1);
        assert_eq!(chunk.coordinate, [-1, -1, -1]);
        assert_eq!(chunk.voxels().len(), CHUNK_EDGE_LENGTH.pow(3));
    }
}
