//! Finite world-space voxel raycasts over the virtual-chunk broad-phase index.
//!
//! Traversal first visits absolute virtual cells, then transforms each candidate
//! chunk ray into local-coordinate space and performs voxel-grid DDA. Candidate
//! chunks may be tested in parallel; equal-distance tie identity is unspecified.

use crate::local_coordinate::{
    data::{AtomicVoxel, CHUNK_EDGE_LENGTH, Chunk, LocalCoordinate},
    virtual_chunk::{
        ChunkReference, VIRTUAL_CHUNK_EDGE_LENGTH, VirtualChunkCoordinate, VirtualChunkIndex,
    },
};
use bevy::{
    ecs::system::SystemParam,
    math::Ray3d,
    prelude::{GlobalTransform, IVec3, Query, Res, Vec3, Vec3A},
    tasks::ComputeTaskPool,
};

const DIRECTION_EPSILON: f32 = 1.0e-7;

/// The nearest nonempty voxel intersected by a finite world-space ray.
///
/// All world-space values use the normalized input ray's distance parameter.
/// The chunk reference is a logical index result and can become stale after ECS
/// mutation; callers should consume it in the frame in which it was produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelRaycastHit {
    /// World-space entry point on the voxel.
    pub point: Vec3,
    /// World-space outward normal of the entered voxel face.
    pub normal: Vec3,
    /// World-space distance from the ray origin.
    pub distance: f32,
    /// Local-coordinate entity and local chunk position owning the voxel.
    pub chunk: ChunkReference,
    /// Voxel position in the owning chunk's `0..CHUNK_EDGE_LENGTH` range.
    pub voxel_relative_position: IVec3,
    /// Atomic voxel identifier at the hit position.
    pub voxel: AtomicVoxel,
    /// Owning-coordinate voxel immediately before entry along the traversal.
    ///
    /// When the ray starts inside a solid voxel, this is chosen opposite the
    /// dominant local ray axis rather than from an actually traversed empty cell.
    pub previous_voxel_position: IVec3,
}

impl VoxelRaycastHit {
    /// Returns the hit voxel position in its owning local coordinate.
    pub fn voxel_position(&self) -> IVec3 {
        self.chunk.local_chunk_position() * CHUNK_EDGE_LENGTH + self.voxel_relative_position
    }
}

/// Read-only ECS system parameter for finite voxel raycasts.
///
/// It reads the latest [`VirtualChunkIndex`] snapshot and resolves candidates
/// against currently live coordinate entities and chunks.
#[derive(SystemParam)]
pub struct VoxelRaycaster<'w, 's> {
    virtual_chunks: Res<'w, VirtualChunkIndex>,
    local_coordinates: Query<'w, 's, (&'static LocalCoordinate, &'static GlobalTransform)>,
}

impl VoxelRaycaster<'_, '_> {
    /// Casts from a camera's world-space translation along its forward axis.
    ///
    /// Returns `None` for an invalid maximum distance or when no indexed nonempty
    /// voxel is intersected.
    pub fn cast_from_camera(
        &self,
        camera_transform: &GlobalTransform,
        max_distance: f32,
    ) -> Option<VoxelRaycastHit> {
        self.cast(
            Ray3d::new(camera_transform.translation(), camera_transform.forward()),
            max_distance,
        )
    }

    /// Returns the nearest hit no farther than `max_distance` world units.
    ///
    /// `max_distance` and the ray origin/direction must be finite, and distance
    /// must be positive. Invalid input, stale broad-phase references, missing or
    /// empty chunks, and non-invertible coordinate transforms are skipped. If
    /// multiple voxels tie exactly, which identity wins is unspecified.
    pub fn cast(&self, ray: Ray3d, max_distance: f32) -> Option<VoxelRaycastHit> {
        raycast_virtual_chunks(&self.virtual_chunks, ray, max_distance, |chunk_reference| {
            let entity = chunk_reference.local_coordinate_entity();
            let coordinate_result = self.local_coordinates.get(entity);
            let (local_coordinate, transform) = coordinate_result.ok()?;
            let chunk_position = chunk_reference.local_chunk_position();
            let chunk = local_coordinate.chunks.get(&chunk_position)?;
            Some(ChunkCandidate {
                chunk_reference,
                chunk,
                transform: *transform,
            })
        })
    }
}

#[derive(Clone, Copy)]
struct ChunkCandidate<'a> {
    chunk_reference: ChunkReference,
    chunk: &'a Chunk,
    transform: GlobalTransform,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RaySegment {
    virtual_coordinate: VirtualChunkCoordinate,
    enter_distance: f32,
    exit_distance: f32,
}

// Traverses cells front-to-back, allowing the first segment-local hit to terminate.
fn raycast_virtual_chunks<'a>(
    index: &VirtualChunkIndex,
    ray: Ray3d,
    max_distance: f32,
    mut resolve_chunk: impl FnMut(ChunkReference) -> Option<ChunkCandidate<'a>>,
) -> Option<VoxelRaycastHit> {
    if !valid_raycast(ray, max_distance) {
        return None;
    }

    for segment in VirtualChunkTraversal::new(ray, max_distance) {
        let Some(chunk_references) = index.chunks_at(segment.virtual_coordinate) else {
            continue;
        };
        let candidates = chunk_references
            .iter()
            .copied()
            .filter_map(&mut resolve_chunk)
            .filter(|candidate| !candidate.chunk.is_empty())
            .collect::<Vec<_>>();

        if let Some(hit) = raycast_chunks(candidates, ray, segment) {
            return Some(hit);
        }
    }

    None
}

fn valid_raycast(ray: Ray3d, max_distance: f32) -> bool {
    max_distance.is_finite()
        && max_distance > 0.0
        && ray.origin.is_finite()
        && ray.direction.as_vec3().is_finite()
}

// Parallelizes only multi-candidate cells and reduces results by world distance.
fn raycast_chunks(
    candidates: Vec<ChunkCandidate<'_>>,
    ray: Ray3d,
    segment: RaySegment,
) -> Option<VoxelRaycastHit> {
    let hits = if candidates.len() > 1 {
        ComputeTaskPool::try_get().map(|task_pool| {
            task_pool.scope(|scope| {
                for candidate in candidates.iter().copied() {
                    scope.spawn(async move { raycast_chunk(candidate, ray, segment) });
                }
            })
        })
    } else {
        None
    };

    hits.unwrap_or_else(|| {
        candidates
            .into_iter()
            .map(|candidate| raycast_chunk(candidate, ray, segment))
            .collect()
    })
    .into_iter()
    .flatten()
    .min_by(|first, second| first.distance.total_cmp(&second.distance))
}

// Preserves the world-ray distance parameter while traversing in local space.
fn raycast_chunk(
    candidate: ChunkCandidate<'_>,
    ray: Ray3d,
    segment: RaySegment,
) -> Option<VoxelRaycastHit> {
    let affine = candidate.transform.affine();
    let determinant = affine.matrix3.determinant();
    if !determinant.is_finite() || determinant.abs() <= f32::EPSILON {
        return None;
    }

    let inverse = affine.inverse();
    let local_origin = inverse.transform_point3(ray.origin);
    let local_direction = inverse.transform_vector3(*ray.direction);
    if !local_origin.is_finite() || !local_direction.is_finite() {
        return None;
    }

    let chunk_origin = candidate.chunk_reference.local_chunk_position() * CHUNK_EDGE_LENGTH;
    let chunk_minimum = chunk_origin.as_vec3();
    let chunk_maximum = chunk_minimum + Vec3::splat(CHUNK_EDGE_LENGTH as f32);
    let intersection =
        ray_aabb_intersection(local_origin, local_direction, chunk_minimum, chunk_maximum)?;
    let enter_distance = intersection
        .enter_distance
        .max(segment.enter_distance)
        .max(0.0);
    let exit_distance = intersection.exit_distance.min(segment.exit_distance);
    if enter_distance >= exit_distance {
        return None;
    }

    let mut voxel_position = ray_grid_position(
        local_origin + local_direction * enter_distance,
        local_direction,
    );
    let mut voxel_relative_position = voxel_position - chunk_origin;
    if !inside_chunk(voxel_relative_position) {
        return None;
    }

    let mut next_boundary = next_grid_boundaries(local_origin, local_direction, voxel_position);
    let boundary_delta = Vec3::new(
        reciprocal_magnitude(local_direction.x),
        reciprocal_magnitude(local_direction.y),
        reciprocal_magnitude(local_direction.z),
    );
    let step = IVec3::new(
        direction_step(local_direction.x),
        direction_step(local_direction.y),
        direction_step(local_direction.z),
    );
    let mut distance = enter_distance;
    let mut local_normal =
        if (enter_distance - intersection.enter_distance).abs() <= DIRECTION_EPSILON {
            intersection.enter_normal
        } else {
            opposite_dominant_axis(local_direction)
        };

    loop {
        if let Some(data) = candidate.chunk.voxel(voxel_relative_position) {
            let world_normal = transformed_normal(inverse, local_normal)?;
            return Some(VoxelRaycastHit {
                point: ray.origin + *ray.direction * distance,
                normal: world_normal,
                distance,
                chunk: candidate.chunk_reference,
                voxel_relative_position,
                voxel: data,
                previous_voxel_position: voxel_position + local_normal.as_ivec3(),
            });
        }

        let next_distance = next_boundary.min_element();
        if !next_distance.is_finite() || next_distance >= exit_distance {
            return None;
        }

        let crossed_axes = [
            boundary_matches(next_boundary.x, next_distance),
            boundary_matches(next_boundary.y, next_distance),
            boundary_matches(next_boundary.z, next_distance),
        ];
        let normal_axis = crossed_axes.iter().position(|crossed| *crossed)?;
        local_normal = axis_vector(normal_axis, -step[normal_axis]);

        for axis in 0..3 {
            if crossed_axes[axis] {
                voxel_position[axis] += step[axis];
                next_boundary[axis] += boundary_delta[axis];
            }
        }
        voxel_relative_position = voxel_position - chunk_origin;
        if !inside_chunk(voxel_relative_position) {
            return None;
        }
        distance = next_distance;
    }
}

#[derive(Clone, Copy)]
struct RayAabbIntersection {
    enter_distance: f32,
    exit_distance: f32,
    enter_normal: Vec3,
}

fn ray_aabb_intersection(
    origin: Vec3,
    direction: Vec3,
    minimum: Vec3,
    maximum: Vec3,
) -> Option<RayAabbIntersection> {
    let mut enter_distance = f32::NEG_INFINITY;
    let mut exit_distance = f32::INFINITY;
    let mut enter_normal = Vec3::ZERO;

    for axis in 0..3 {
        if direction[axis].abs() <= DIRECTION_EPSILON {
            if origin[axis] < minimum[axis] || origin[axis] > maximum[axis] {
                return None;
            }
            continue;
        }

        let (near_distance, far_distance, near_normal) = if direction[axis] > 0.0 {
            (
                (minimum[axis] - origin[axis]) / direction[axis],
                (maximum[axis] - origin[axis]) / direction[axis],
                axis_vector(axis, -1),
            )
        } else {
            (
                (maximum[axis] - origin[axis]) / direction[axis],
                (minimum[axis] - origin[axis]) / direction[axis],
                axis_vector(axis, 1),
            )
        };

        if near_distance > enter_distance {
            enter_distance = near_distance;
            enter_normal = near_normal;
        }
        exit_distance = exit_distance.min(far_distance);
        if enter_distance > exit_distance {
            return None;
        }
    }

    Some(RayAabbIntersection {
        enter_distance,
        exit_distance,
        enter_normal,
    })
}

fn transformed_normal(inverse: bevy::math::Affine3A, local_normal: Vec3) -> Option<Vec3> {
    let normal = inverse.matrix3.transpose() * Vec3A::from(local_normal);
    Vec3::from(normal).try_normalize()
}

fn ray_grid_position(position: Vec3, direction: Vec3) -> IVec3 {
    IVec3::new(
        grid_coordinate(position.x, direction.x),
        grid_coordinate(position.y, direction.y),
        grid_coordinate(position.z, direction.z),
    )
}

fn grid_coordinate(position: f32, direction: f32) -> i32 {
    let floor = position.floor();
    let coordinate = floor as i32;
    if direction < 0.0 && position == floor {
        coordinate.saturating_sub(1)
    } else {
        coordinate
    }
}

fn next_grid_boundaries(origin: Vec3, direction: Vec3, position: IVec3) -> Vec3 {
    Vec3::new(
        next_grid_boundary(origin.x, direction.x, position.x),
        next_grid_boundary(origin.y, direction.y, position.y),
        next_grid_boundary(origin.z, direction.z, position.z),
    )
}

fn next_grid_boundary(origin: f32, direction: f32, position: i32) -> f32 {
    if direction > DIRECTION_EPSILON {
        (position.saturating_add(1) as f32 - origin) / direction
    } else if direction < -DIRECTION_EPSILON {
        (position as f32 - origin) / direction
    } else {
        f32::INFINITY
    }
}

fn reciprocal_magnitude(direction: f32) -> f32 {
    if direction.abs() > DIRECTION_EPSILON {
        direction.abs().recip()
    } else {
        f32::INFINITY
    }
}

fn direction_step(direction: f32) -> i32 {
    if direction > DIRECTION_EPSILON {
        1
    } else if direction < -DIRECTION_EPSILON {
        -1
    } else {
        0
    }
}

fn boundary_matches(boundary: f32, minimum: f32) -> bool {
    (boundary - minimum).abs() <= DIRECTION_EPSILON * minimum.abs().max(1.0)
}

fn inside_chunk(position: IVec3) -> bool {
    (0..CHUNK_EDGE_LENGTH).contains(&position.x)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.y)
        && (0..CHUNK_EDGE_LENGTH).contains(&position.z)
}

fn opposite_dominant_axis(direction: Vec3) -> Vec3 {
    let absolute = direction.abs();
    let axis = if absolute.x >= absolute.y && absolute.x >= absolute.z {
        0
    } else if absolute.y >= absolute.z {
        1
    } else {
        2
    };
    axis_vector(axis, -direction[axis].signum() as i32)
}

fn axis_vector(axis: usize, direction: i32) -> Vec3 {
    let mut vector = Vec3::ZERO;
    vector[axis] = direction as f32;
    vector
}

/// Front-to-back DDA over absolute virtual cells, clipped to a finite distance.
struct VirtualChunkTraversal {
    max_distance: f32,
    coordinate: VirtualChunkCoordinate,
    step: [i64; 3],
    next_boundary: [f32; 3],
    boundary_delta: [f32; 3],
    enter_distance: f32,
    finished: bool,
}

impl VirtualChunkTraversal {
    fn new(ray: Ray3d, max_distance: f32) -> Self {
        let direction = *ray.direction;
        let edge = VIRTUAL_CHUNK_EDGE_LENGTH as f32;
        let coordinate = [
            grid_coordinate(ray.origin.x / edge, direction.x) as i64,
            grid_coordinate(ray.origin.y / edge, direction.y) as i64,
            grid_coordinate(ray.origin.z / edge, direction.z) as i64,
        ];
        let mut step = [0; 3];
        let mut next_boundary = [f32::INFINITY; 3];
        let mut boundary_delta = [f32::INFINITY; 3];

        for axis in 0..3 {
            if direction[axis] > DIRECTION_EPSILON {
                step[axis] = 1;
                let boundary = coordinate[axis].saturating_add(1) as f32 * edge;
                next_boundary[axis] = (boundary - ray.origin[axis]) / direction[axis];
                boundary_delta[axis] = edge / direction[axis];
            } else if direction[axis] < -DIRECTION_EPSILON {
                step[axis] = -1;
                let boundary = coordinate[axis] as f32 * edge;
                next_boundary[axis] = (boundary - ray.origin[axis]) / direction[axis];
                boundary_delta[axis] = -edge / direction[axis];
            }
        }

        Self {
            max_distance,
            coordinate,
            step,
            next_boundary,
            boundary_delta,
            enter_distance: 0.0,
            finished: false,
        }
    }
}

impl Iterator for VirtualChunkTraversal {
    type Item = RaySegment;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let exit_distance = self
            .next_boundary
            .into_iter()
            .fold(self.max_distance, f32::min)
            .max(self.enter_distance);
        let segment = RaySegment {
            virtual_coordinate: self.coordinate,
            enter_distance: self.enter_distance,
            exit_distance,
        };

        if exit_distance >= self.max_distance {
            self.finished = true;
            return Some(segment);
        }

        for axis in 0..3 {
            if boundary_matches(self.next_boundary[axis], exit_distance) {
                self.coordinate[axis] = self.coordinate[axis].saturating_add(self.step[axis]);
                self.next_boundary[axis] += self.boundary_delta[axis];
            }
        }
        self.enter_distance = exit_distance;
        Some(segment)
    }
}

#[cfg(test)]
mod tests;
