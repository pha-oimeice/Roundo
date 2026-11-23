//! Deterministic three-dimensional maze generation from a seed and radius.

use super::{
    CHUNK_EDGE_LENGTH, ChunkCoordinate, ChunkGenerator, EMPTY_MATERIAL_ID, GeneratedChunk,
    SOLID_MATERIAL_ID, chunk_generator::voxel_index,
};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::{collections::VecDeque, ops::Range};

/// A finite seeded maze. The voxel volume starts at `[0, 0, 0]`; space outside it is empty.
pub struct MazeGenerator {
    walls: Vec<Vec<Vec<bool>>>,
}

impl MazeGenerator {
    /// Builds the entire maze once so chunk queries preserve global connectivity.
    /// Panics if `radius` is smaller than two or the diameter exceeds `i16::MAX`.
    pub fn new(seed: u64, radius: i16) -> Self {
        assert!(radius >= 2, "radius must be greater than 1");
        let size = radius
            .checked_mul(2)
            .and_then(|diameter| diameter.checked_add(1))
            .expect("maze diameter must fit i16") as usize;
        Self {
            walls: generate_occupancy(seed, size),
        }
    }

    /// Enumerates the chunks intersecting the finite maze volume.
    pub fn chunk_coordinates(&self) -> impl Iterator<Item = ChunkCoordinate> {
        let axis = self.chunk_axis();
        let end = axis.end;
        axis.flat_map(move |x| (0..end).flat_map(move |y| (0..end).map(move |z| [x, y, z])))
    }

    fn chunk_axis(&self) -> Range<i64> {
        0..self.walls.len().div_ceil(CHUNK_EDGE_LENGTH) as i64
    }
}

impl ChunkGenerator for MazeGenerator {
    fn generate_chunk(&self, coordinate: ChunkCoordinate) -> GeneratedChunk {
        let axis = self.chunk_axis();
        if coordinate.iter().any(|position| !axis.contains(position)) {
            return GeneratedChunk::default();
        }

        let [origin_x, origin_y, origin_z] =
            coordinate.map(|position| position as usize * CHUNK_EDGE_LENGTH);
        let [size_x, size_y, size_z] = [origin_x, origin_y, origin_z]
            .map(|origin| (self.walls.len() - origin).min(CHUNK_EDGE_LENGTH));
        let mut voxels = vec![EMPTY_MATERIAL_ID; CHUNK_EDGE_LENGTH.pow(3)];
        for x in 0..size_x {
            for y in 0..size_y {
                for z in 0..size_z {
                    // Preserve the original maze output: open cells become solid voxels.
                    if !self.walls[origin_x + x][origin_y + y][origin_z + z] {
                        voxels[voxel_index([x, y, z])] = SOLID_MATERIAL_ID;
                    }
                }
            }
        }
        GeneratedChunk::from_voxels(voxels)
            .expect("the maze generator always emits one complete chunk")
    }
}

// Smooths seeded occupancy before retaining the largest open region and adding openings.
fn generate_occupancy(seed: u64, size: usize) -> Vec<Vec<Vec<bool>>> {
    assert!(size >= 3);

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut maze = vec![vec![vec![true; size]; size]; size];

    for plane in &mut maze {
        for row in plane {
            for cell in row {
                *cell = rng.random_bool(0.45);
            }
        }
    }

    for _ in 0..5 {
        let previous = maze.clone();
        for (x, plane) in maze.iter_mut().enumerate() {
            for (y, row) in plane.iter_mut().enumerate() {
                for (z, cell) in row.iter_mut().enumerate() {
                    let mut wall_count = 0;
                    for dx in -1_i32..=1 {
                        for dy in -1_i32..=1 {
                            for dz in -1_i32..=1 {
                                if dx == 0 && dy == 0 && dz == 0 {
                                    continue;
                                }

                                let neighbor = [x as i32 + dx, y as i32 + dy, z as i32 + dz];
                                let outside = neighbor.iter().any(|value| *value < 0)
                                    || neighbor.iter().any(|value| *value >= size as i32);
                                if outside
                                    || previous[neighbor[0] as usize][neighbor[1] as usize]
                                        [neighbor[2] as usize]
                                {
                                    wall_count += 1;
                                }
                            }
                        }
                    }
                    *cell = wall_count >= 14;
                }
            }
        }
    }

    keep_largest_open_region(&mut maze);
    add_loops(&mut maze, &mut rng);
    add_boundary_exits(&mut maze, &mut rng);
    maze
}

// Removes disconnected cavities so every retained cell is traversable.
fn keep_largest_open_region(maze: &mut [Vec<Vec<bool>>]) {
    let size = maze.len();
    let mut visited = vec![vec![vec![false; size]; size]; size];
    let mut largest = Vec::new();

    for start_x in 0..size {
        for start_y in 0..size {
            for start_z in 0..size {
                if maze[start_x][start_y][start_z] || visited[start_x][start_y][start_z] {
                    continue;
                }

                let mut region = Vec::new();
                let mut queue = VecDeque::from([(start_x, start_y, start_z)]);
                visited[start_x][start_y][start_z] = true;

                while let Some((x, y, z)) = queue.pop_front() {
                    region.push((x, y, z));
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        let neighbor = [x as i32 + dx, y as i32 + dy, z as i32 + dz];
                        if neighbor.iter().any(|value| *value < 0)
                            || neighbor.iter().any(|value| *value >= size as i32)
                        {
                            continue;
                        }
                        let [neighbor_x, neighbor_y, neighbor_z] =
                            neighbor.map(|value| value as usize);
                        if !visited[neighbor_x][neighbor_y][neighbor_z]
                            && !maze[neighbor_x][neighbor_y][neighbor_z]
                        {
                            visited[neighbor_x][neighbor_y][neighbor_z] = true;
                            queue.push_back((neighbor_x, neighbor_y, neighbor_z));
                        }
                    }
                }

                if region.len() > largest.len() {
                    largest = region;
                }
            }
        }
    }

    for plane in maze.iter_mut() {
        for row in plane {
            row.fill(true);
        }
    }
    for (x, y, z) in largest {
        maze[x][y][z] = false;
    }
}

// Opens selected internal walls to reduce strictly linear paths.
fn add_loops(maze: &mut [Vec<Vec<bool>>], rng: &mut ChaCha8Rng) {
    let size = maze.len();
    for _ in 0..size * size * size / 200 {
        let x = rng.random_range(1..size - 1);
        let y = rng.random_range(1..size - 1);
        let z = rng.random_range(1..size - 1);
        if !maze[x][y][z] {
            continue;
        }

        for (first, second) in [
            ((1, 0, 0), (-1, 0, 0)),
            ((0, 1, 0), (0, -1, 0)),
            ((0, 0, 1), (0, 0, -1)),
        ] {
            let first = offset_index((x, y, z), first);
            let second = offset_index((x, y, z), second);
            if !maze[first.0][first.1][first.2] && !maze[second.0][second.1][second.2] {
                maze[x][y][z] = false;
                break;
            }
        }
    }
}

// Adds exterior openings reachable from the retained region.
fn add_boundary_exits(maze: &mut [Vec<Vec<bool>>], rng: &mut ChaCha8Rng) {
    let size = maze.len();
    for _ in 0..6.max(size / 4) {
        let first = rng.random_range(0..size);
        let second = rng.random_range(0..size);
        let position = match rng.random_range(0..6) {
            0 => (0, first, second),
            1 => (size - 1, first, second),
            2 => (first, 0, second),
            3 => (first, size - 1, second),
            4 => (first, second, 0),
            _ => (first, second, size - 1),
        };
        maze[position.0][position.1][position.2] = false;
    }
}

// Applies signed neighbor offsets after callers enforce interior bounds.
fn offset_index(position: (usize, usize, usize), offset: (i32, i32, i32)) -> (usize, usize, usize) {
    (
        (position.0 as i32 + offset.0) as usize,
        (position.1 as i32 + offset.1) as usize,
        (position.2 as i32 + offset.2) as usize,
    )
}
