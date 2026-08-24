use crate::local_coordinate::data::{CHUNK_EDGE_LENGTH, LocalCoordinate};
use bevy::prelude::{Entity, GlobalTransform, IVec3, Query, ResMut, Resource, Vec3};
use std::collections::{HashMap, HashSet};

pub const VIRTUAL_CHUNK_EDGE_LENGTH: i64 = CHUNK_EDGE_LENGTH as i64;
pub type VirtualChunkCoordinate = [i64; 3];

/// An opaque reference from an absolute-space virtual chunk to one local chunk.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChunkReference {
    local_coordinate_entity: Entity,
    local_chunk_position: IVec3,
}

impl ChunkReference {
    pub(crate) const fn new(local_coordinate_entity: Entity, local_chunk_position: IVec3) -> Self {
        Self {
            local_coordinate_entity,
            local_chunk_position,
        }
    }

    /// Returns the Bevy entity owning this chunk.
    pub const fn local_coordinate_entity(self) -> Entity {
        self.local_coordinate_entity
    }

    /// Returns the chunk position in its owning local coordinate.
    pub const fn local_chunk_position(self) -> IVec3 {
        self.local_chunk_position
    }
}

/// Derived absolute-space lookup for chunks owned by every local coordinate.
#[derive(Resource, Default)]
pub struct VirtualChunkIndex {
    chunks: HashMap<VirtualChunkCoordinate, HashSet<ChunkReference>>,
}

impl VirtualChunkIndex {
    /// Returns all known chunks stored in virtual chunks intersecting the sphere.
    ///
    /// Callers only provide an absolute-space `f(x, y, z, radius)` query. Ownership
    /// by a local coordinate remains an implementation detail of the returned
    /// references.
    pub fn chunks_in_radius(&self, x: f64, y: f64, z: f64, radius: f64) -> HashSet<ChunkReference> {
        virtual_chunks_intersecting_radius([x, y, z], radius)
            .into_iter()
            .filter_map(|coordinate| self.chunks.get(&coordinate))
            .flat_map(|chunks| chunks.iter().copied())
            .collect()
    }

    pub(crate) fn chunks_at(
        &self,
        coordinate: VirtualChunkCoordinate,
    ) -> Option<&HashSet<ChunkReference>> {
        self.chunks.get(&coordinate)
    }

    fn clear(&mut self) {
        self.chunks.clear();
    }

    pub(crate) fn insert(
        &mut self,
        local_coordinate_entity: Entity,
        local_chunk_position: IVec3,
        transform: &GlobalTransform,
    ) {
        let chunk_reference = ChunkReference::new(local_coordinate_entity, local_chunk_position);
        for virtual_coordinate in virtual_coordinates_for_chunk(local_chunk_position, transform) {
            self.chunks
                .entry(virtual_coordinate)
                .or_default()
                .insert(chunk_reference);
        }
    }
}

pub(crate) fn rebuild_virtual_chunk_index(
    mut index: ResMut<VirtualChunkIndex>,
    local_coordinates: Query<(Entity, &LocalCoordinate, &GlobalTransform)>,
) {
    index.clear();
    for (entity, local_coordinate, transform) in &local_coordinates {
        for local_chunk_position in local_coordinate.chunks.keys().copied() {
            index.insert(entity, local_chunk_position, transform);
        }
    }
}

fn virtual_coordinates_for_chunk(
    local_chunk_position: IVec3,
    transform: &GlobalTransform,
) -> Vec<VirtualChunkCoordinate> {
    let local_minimum = local_chunk_position.as_vec3() * CHUNK_EDGE_LENGTH as f32;
    let local_maximum = local_minimum + Vec3::splat(CHUNK_EDGE_LENGTH as f32);
    let mut absolute_minimum = Vec3::splat(f32::INFINITY);
    let mut absolute_maximum = Vec3::splat(f32::NEG_INFINITY);

    for x in [local_minimum.x, local_maximum.x] {
        for y in [local_minimum.y, local_maximum.y] {
            for z in [local_minimum.z, local_maximum.z] {
                let corner = transform.transform_point(Vec3::new(x, y, z));
                absolute_minimum = absolute_minimum.min(corner);
                absolute_maximum = absolute_maximum.max(corner);
            }
        }
    }

    if !absolute_minimum.is_finite() || !absolute_maximum.is_finite() {
        return Vec::new();
    }

    let edge = VIRTUAL_CHUNK_EDGE_LENGTH as f32;
    let minimum = [
        (absolute_minimum.x / edge).floor() as i64,
        (absolute_minimum.y / edge).floor() as i64,
        (absolute_minimum.z / edge).floor() as i64,
    ];
    let maximum = [
        (absolute_maximum.x.next_down() / edge).floor() as i64,
        (absolute_maximum.y.next_down() / edge).floor() as i64,
        (absolute_maximum.z.next_down() / edge).floor() as i64,
    ];
    let mut coordinates = Vec::new();

    for x in minimum[0]..=maximum[0] {
        for y in minimum[1]..=maximum[1] {
            for z in minimum[2]..=maximum[2] {
                coordinates.push([x, y, z]);
            }
        }
    }

    coordinates
}

fn virtual_chunks_intersecting_radius(
    center: [f64; 3],
    radius: f64,
) -> Vec<VirtualChunkCoordinate> {
    if !radius.is_finite()
        || radius < 0.0
        || center.iter().any(|coordinate| !coordinate.is_finite())
    {
        return Vec::new();
    }

    let edge = VIRTUAL_CHUNK_EDGE_LENGTH as f64;
    let minimum = center.map(|coordinate| ((coordinate - radius) / edge).floor() as i64);
    let maximum = center.map(|coordinate| ((coordinate + radius) / edge).floor() as i64);
    let radius_squared = radius * radius;
    let mut virtual_chunks = Vec::new();

    for x in minimum[0]..=maximum[0] {
        for y in minimum[1]..=maximum[1] {
            for z in minimum[2]..=maximum[2] {
                let coordinate = [x, y, z];
                let distance_squared = (0..3)
                    .map(|axis| {
                        let chunk_minimum = coordinate[axis] as f64 * edge;
                        let chunk_maximum = chunk_minimum + edge;
                        if center[axis] < chunk_minimum {
                            (chunk_minimum - center[axis]).powi(2)
                        } else if center[axis] > chunk_maximum {
                            (center[axis] - chunk_maximum).powi(2)
                        } else {
                            0.0
                        }
                    })
                    .sum::<f64>();
                if distance_squared <= radius_squared {
                    virtual_chunks.push(coordinate);
                }
            }
        }
    }

    virtual_chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{Quat, Transform, Vec3};

    #[test]
    fn indexes_the_transformed_floor_of_a_chunk_center() {
        let mut world = bevy::prelude::World::new();
        let entity = world.spawn_empty().id();
        let mut index = VirtualChunkIndex::default();
        index.insert(
            entity,
            IVec3::new(-1, 0, 0),
            &GlobalTransform::from(Transform::from_translation(Vec3::new(24.0, 0.0, 0.0))),
        );

        assert_eq!(
            index.chunks_in_radius(16.0, 8.0, 8.0, 0.0),
            HashSet::from([ChunkReference::new(entity, IVec3::new(-1, 0, 0))])
        );
    }

    #[test]
    fn indexes_every_virtual_chunk_overlapped_by_a_rotated_chunk() {
        let mut world = bevy::prelude::World::new();
        let entity = world.spawn_empty().id();
        let chunk = ChunkReference::new(entity, IVec3::ZERO);
        let mut index = VirtualChunkIndex::default();
        index.insert(
            entity,
            IVec3::ZERO,
            &GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_z(
                std::f32::consts::FRAC_PI_4,
            ))),
        );

        assert!(
            index
                .chunks_at([-1, 0, 0])
                .is_some_and(|chunks| chunks.contains(&chunk))
        );
        assert!(
            index
                .chunks_at([0, 1, 0])
                .is_some_and(|chunks| chunks.contains(&chunk))
        );
    }

    #[test]
    fn radius_query_includes_every_intersected_virtual_chunk() {
        let mut world = bevy::prelude::World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        let mut index = VirtualChunkIndex::default();
        index.insert(first, IVec3::ZERO, &GlobalTransform::default());
        index.insert(second, IVec3::X, &GlobalTransform::default());

        assert_eq!(
            index.chunks_in_radius(16.0, 8.0, 8.0, 1.0),
            HashSet::from([
                ChunkReference::new(first, IVec3::ZERO),
                ChunkReference::new(second, IVec3::X),
            ])
        );
    }

    #[test]
    fn rejects_invalid_queries() {
        let index = VirtualChunkIndex::default();
        assert!(index.chunks_in_radius(0.0, 0.0, 0.0, -1.0).is_empty());
        assert!(index.chunks_in_radius(f64::NAN, 0.0, 0.0, 1.0).is_empty());
    }
}
