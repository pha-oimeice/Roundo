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
    references: HashMap<ChunkReference, Vec<VirtualChunkCoordinate>>,
    entity_transforms: HashMap<Entity, [f32; 12]>,
}

impl VirtualChunkIndex {
    /// Returns all known chunks stored in virtual chunks intersecting the sphere.
    ///
    /// Callers only provide an absolute-space `f(x, y, z, radius)` query. Ownership
    /// by a local coordinate remains an implementation detail of the returned
    /// references.
    pub fn chunks_in_radius(&self, x: f64, y: f64, z: f64, radius: f64) -> HashSet<ChunkReference> {
        let center = [x, y, z];
        if !radius.is_finite()
            || radius < 0.0
            || center.iter().any(|coordinate| !coordinate.is_finite())
        {
            return HashSet::new();
        }
        let edge = VIRTUAL_CHUNK_EDGE_LENGTH as f64;
        let diameter_in_chunks = (radius * 2.0 / edge).ceil() as usize + 2;
        let query_volume = diameter_in_chunks.saturating_pow(3);
        if query_volume > self.chunks.len() {
            return self
                .chunks
                .iter()
                .filter(|(coordinate, _)| {
                    virtual_chunk_intersects_radius(**coordinate, center, radius)
                })
                .flat_map(|(_, chunks)| chunks.iter().copied())
                .collect();
        }
        virtual_chunks_intersecting_radius(center, radius)
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

    fn remove_reference(&mut self, chunk_reference: ChunkReference) {
        let Some(coordinates) = self.references.remove(&chunk_reference) else {
            return;
        };
        for coordinate in coordinates {
            if let Some(chunks) = self.chunks.get_mut(&coordinate) {
                chunks.remove(&chunk_reference);
                if chunks.is_empty() {
                    self.chunks.remove(&coordinate);
                }
            }
        }
    }

    fn remove_entity(&mut self, entity: Entity) {
        let references = self
            .references
            .keys()
            .filter(|reference| reference.local_coordinate_entity == entity)
            .copied()
            .collect::<Vec<_>>();
        for reference in references {
            self.remove_reference(reference);
        }
        self.entity_transforms.remove(&entity);
    }

    pub(crate) fn insert(
        &mut self,
        local_coordinate_entity: Entity,
        local_chunk_position: IVec3,
        transform: &GlobalTransform,
    ) {
        let chunk_reference = ChunkReference::new(local_coordinate_entity, local_chunk_position);
        self.remove_reference(chunk_reference);
        let coordinates = virtual_coordinates_for_chunk(local_chunk_position, transform);
        for &virtual_coordinate in &coordinates {
            self.chunks
                .entry(virtual_coordinate)
                .or_default()
                .insert(chunk_reference);
        }
        self.references.insert(chunk_reference, coordinates);
    }
}

pub(crate) fn rebuild_virtual_chunk_index(
    mut index: ResMut<VirtualChunkIndex>,
    local_coordinates: Query<(Entity, &LocalCoordinate, &GlobalTransform)>,
) {
    let live_entities = local_coordinates
        .iter()
        .map(|(entity, _, _)| entity)
        .collect::<HashSet<_>>();
    let stale_entities = index
        .entity_transforms
        .keys()
        .filter(|entity| !live_entities.contains(entity))
        .copied()
        .collect::<Vec<_>>();
    for entity in stale_entities {
        index.remove_entity(entity);
    }

    for (entity, local_coordinate, transform) in &local_coordinates {
        let transform_key = transform.affine().to_cols_array();
        if index.entity_transforms.get(&entity) != Some(&transform_key) {
            index.remove_entity(entity);
            for position in local_coordinate.chunks.keys().copied() {
                index.insert(entity, position, transform);
            }
            index.entity_transforms.insert(entity, transform_key);
            continue;
        }
        for &position in &local_coordinate.changed_chunks {
            let reference = ChunkReference::new(entity, position);
            index.remove_reference(reference);
            if local_coordinate.chunks.contains_key(&position) {
                index.insert(entity, position, transform);
            }
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

fn virtual_chunk_intersects_radius(
    coordinate: VirtualChunkCoordinate,
    center: [f64; 3],
    radius: f64,
) -> bool {
    let edge = VIRTUAL_CHUNK_EDGE_LENGTH as f64;
    let distance_squared = (0..3)
        .map(|axis| {
            let minimum = coordinate[axis] as f64 * edge;
            let maximum = minimum + edge;
            if center[axis] < minimum {
                (minimum - center[axis]).powi(2)
            } else if center[axis] > maximum {
                (center[axis] - maximum).powi(2)
            } else {
                0.0
            }
        })
        .sum::<f64>();
    distance_squared <= radius * radius
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
    let mut virtual_chunks = Vec::new();

    for x in minimum[0]..=maximum[0] {
        for y in minimum[1]..=maximum[1] {
            for z in minimum[2]..=maximum[2] {
                let coordinate = [x, y, z];
                if virtual_chunk_intersects_radius(coordinate, center, radius) {
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
    fn incrementally_tracks_changed_and_removed_chunks() {
        use bevy::prelude::{App, Update};
        let mut coordinate = LocalCoordinate::default();
        coordinate.mark_chunk_loaded(IVec3::ZERO);
        let mut app = App::new();
        app.init_resource::<VirtualChunkIndex>()
            .add_systems(Update, rebuild_virtual_chunk_index);
        let entity = app
            .world_mut()
            .spawn((coordinate, GlobalTransform::default()))
            .id();
        app.update();
        assert!(
            app.world()
                .resource::<VirtualChunkIndex>()
                .chunks_at([0, 0, 0])
                .is_some()
        );

        {
            let mut coordinate = app.world_mut().get_mut::<LocalCoordinate>(entity).unwrap();
            coordinate.changed_chunks.clear();
            coordinate.mark_chunk_loaded(IVec3::X);
            coordinate.remove_chunk(IVec3::ZERO);
        }
        app.update();
        let index = app.world().resource::<VirtualChunkIndex>();
        assert!(index.chunks_at([0, 0, 0]).is_none());
        assert!(index.chunks_at([1, 0, 0]).is_some());
    }

    #[test]
    fn rejects_invalid_queries() {
        let index = VirtualChunkIndex::default();
        assert!(index.chunks_in_radius(0.0, 0.0, 0.0, -1.0).is_empty());
        assert!(index.chunks_in_radius(f64::NAN, 0.0, 0.0, 1.0).is_empty());
    }
}
