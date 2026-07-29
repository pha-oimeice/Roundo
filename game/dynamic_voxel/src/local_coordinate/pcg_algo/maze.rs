use crate::local_coordinate::data::{AtomicVoxel, AtomicVoxelData};
use bevy::math::IVec3;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::VecDeque;
/// 3d cuboid maze generation
pub fn generate_maze(seed: u64, radius: i16) -> Vec<AtomicVoxel> {
    if radius < 2 {
        panic!("radius must be greater than 1");
    }
    let maze_size = radius * 2 + 1;
    let mut data = Vec::<AtomicVoxel>::new();
    let temp = gpt_maze_algo_v0(seed, maze_size as usize);
    let x_size = temp.len();
    let y_size = temp[0].len();
    let z_size = temp[0][0].len();
    for x in 0..x_size {
        for y in 0..y_size {
            for z in 0..z_size {
                if !temp[x][y][z] {
                    data.push(AtomicVoxel {
                        position: IVec3::new(x as i32, y as i32, z as i32),
                        data: AtomicVoxelData { id: 1 },
                    });
                }
            }
        }
    }
    data
}

fn gpt_maze_algo_v0(seed: u64, size: usize) -> Vec<Vec<Vec<bool>>> {
    assert!(size >= 3);

    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // true = wall
    let mut maze = vec![vec![vec![true; size]; size]; size];

    // ------------------------
    // Step1. Random initialize
    // ------------------------
    for x in 0..size {
        for y in 0..size {
            for z in 0..size {
                maze[x][y][z] = rng.random_bool(0.45);
            }
        }
    }

    // ------------------------
    // Step2. Cellular Automata
    // ------------------------
    for _ in 0..5 {
        let old = maze.clone();
        for x in 0..size {
            for y in 0..size {
                for z in 0..size {
                    let mut wall_count = 0;
                    for dx in -1i32..=1 {
                        for dy in -1i32..=1 {
                            for dz in -1i32..=1 {
                                if dx == 0 && dy == 0 && dz == 0 {
                                    continue;
                                }

                                let nx = x as i32 + dx;
                                let ny = y as i32 + dy;
                                let nz = z as i32 + dz;

                                if nx < 0
                                    || ny < 0
                                    || nz < 0
                                    || nx >= size as i32
                                    || ny >= size as i32
                                    || nz >= size as i32
                                {
                                    wall_count += 1;
                                } else if old[nx as usize][ny as usize][nz as usize] {
                                    wall_count += 1;
                                }
                            }
                        }
                    }
                    maze[x][y][z] = wall_count >= 14;
                }
            }
        }
    }

    // ------------------------
    // Step3. Keep largest region
    // ------------------------

    let mut visited = vec![vec![vec![false; size]; size]; size];

    let mut largest = Vec::new();

    for sx in 0..size {
        for sy in 0..size {
            for sz in 0..size {
                if maze[sx][sy][sz] || visited[sx][sy][sz] {
                    continue;
                }

                let mut region = Vec::new();
                let mut queue = VecDeque::new();

                queue.push_back((sx, sy, sz));
                visited[sx][sy][sz] = true;

                while let Some((x, y, z)) = queue.pop_front() {
                    region.push((x, y, z));

                    let dirs = [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ];

                    for (dx, dy, dz) in dirs {
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        let nz = z as i32 + dz;

                        if nx < 0
                            || ny < 0
                            || nz < 0
                            || nx >= size as i32
                            || ny >= size as i32
                            || nz >= size as i32
                        {
                            continue;
                        }

                        let nx = nx as usize;
                        let ny = ny as usize;
                        let nz = nz as usize;

                        if visited[nx][ny][nz] {
                            continue;
                        }

                        if maze[nx][ny][nz] {
                            continue;
                        }

                        visited[nx][ny][nz] = true;
                        queue.push_back((nx, ny, nz));
                    }
                }

                if region.len() > largest.len() {
                    largest = region;
                }
            }
        }
    }

    // 全部变墙
    for x in 0..size {
        for y in 0..size {
            for z in 0..size {
                maze[x][y][z] = true;
            }
        }
    }

    // 保留最大连通块
    for (x, y, z) in &largest {
        maze[*x][*y][*z] = false;
    }

    // ------------------------
    // Step4. Add loops
    // ------------------------

    let extra = size * size * size / 200;

    for _ in 0..extra {
        let x = rng.random_range(1..size - 1);
        let y = rng.random_range(1..size - 1);
        let z = rng.random_range(1..size - 1);

        if !maze[x][y][z] {
            continue;
        }

        let pairs = [
            ((1, 0, 0), (-1, 0, 0)),
            ((0, 1, 0), (0, -1, 0)),
            ((0, 0, 1), (0, 0, -1)),
        ];

        for (a, b) in pairs {
            let ax = (x as i32 + a.0) as usize;
            let ay = (y as i32 + a.1) as usize;
            let az = (z as i32 + a.2) as usize;

            let bx = (x as i32 + b.0) as usize;
            let by = (y as i32 + b.1) as usize;
            let bz = (z as i32 + b.2) as usize;

            if !maze[ax][ay][az] && !maze[bx][by][bz] {
                maze[x][y][z] = false;
                break;
            }
        }
    }

    // ------------------------
    // Step5. Boundary exits
    // ------------------------

    let exits = 6.max(size / 4);

    for _ in 0..exits {
        let face = rng.random_range(0..6);

        let (x, y, z) = match face {
            0 => (0, rng.random_range(0..size), rng.random_range(0..size)),

            1 => (
                size - 1,
                rng.random_range(0..size),
                rng.random_range(0..size),
            ),

            2 => (rng.random_range(0..size), 0, rng.random_range(0..size)),

            3 => (
                rng.random_range(0..size),
                size - 1,
                rng.random_range(0..size),
            ),

            4 => (rng.random_range(0..size), rng.random_range(0..size), 0),

            _ => (
                rng.random_range(0..size),
                rng.random_range(0..size),
                size - 1,
            ),
        };
        maze[x][y][z] = false;
    }
    maze
}
