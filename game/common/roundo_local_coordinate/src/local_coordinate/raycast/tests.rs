use super::*;

fn test_candidate<'a>(
    coordinates: &'a HashMap<Entity, (LocalCoordinate, GlobalTransform)>,
    chunk_reference: ChunkReference,
) -> Option<ChunkCandidate<'a>> {
    let entity = chunk_reference.local_coordinate_entity();
    let (local_coordinate, transform) = coordinates.get(&entity)?;
    let position = chunk_reference.local_chunk_position();
    let chunk = local_coordinate.chunks.get(&position)?;
    Some(ChunkCandidate {
        chunk_reference,
        chunk,
        transform: *transform,
    })
}
use crate::local_coordinate::data::{LocalCoordinate, PositionedAtomicVoxel, SOLID_VOXEL_ID};
use bevy::{
    prelude::{Dir3, Entity, Quat, Transform},
    tasks::TaskPool,
};
use std::collections::HashMap;

fn solid_voxel(position: IVec3) -> PositionedAtomicVoxel {
    PositionedAtomicVoxel {
        position,
        voxel: SOLID_VOXEL_ID,
    }
}

#[test]
fn returns_the_first_voxel_with_point_normal_and_chunk_information() {
    let mut world = bevy::prelude::World::new();
    let entity = world.spawn_empty().id();
    let local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(1, 2, 3)),
        solid_voxel(IVec3::new(4, 2, 3)),
    ]);
    let transform = GlobalTransform::default();
    let mut index = VirtualChunkIndex::default();
    index.insert(entity, IVec3::ZERO, &transform);
    let coordinates = HashMap::from([(entity, (local_coordinate, transform))]);

    let hit = raycast_virtual_chunks(
        &index,
        Ray3d::new(Vec3::new(-1.0, 2.5, 3.5), Dir3::X),
        10.0,
        |chunk_reference| test_candidate(&coordinates, chunk_reference),
    )
    .expect("ray should hit the nearest solid voxel");

    assert_eq!(hit.point, Vec3::new(1.0, 2.5, 3.5));
    assert_eq!(hit.normal, Vec3::NEG_X);
    assert_eq!(hit.distance, 2.0);
    assert_eq!(hit.chunk.local_coordinate_entity(), entity);
    assert_eq!(hit.chunk.local_chunk_position(), IVec3::ZERO);
    assert_eq!(hit.voxel_relative_position, IVec3::new(1, 2, 3));
    assert_eq!(hit.voxel, SOLID_VOXEL_ID);
    assert_eq!(hit.voxel_position(), IVec3::new(1, 2, 3));
    assert_eq!(hit.previous_voxel_position, IVec3::new(0, 2, 3));
}

#[test]
fn rotates_the_ray_into_chunk_space_and_the_normal_back_to_world_space() {
    let mut world = bevy::prelude::World::new();
    let entity = world.spawn_empty().id();
    let local_coordinate = LocalCoordinate::from_voxels([solid_voxel(IVec3::ZERO)]);
    let transform = GlobalTransform::from(
        Transform::from_translation(Vec3::new(8.0, 4.0, 2.0))
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
    );
    let local_origin = Vec3::new(-2.0, 0.5, 0.5);
    let world_origin = transform.transform_point(local_origin);
    let world_direction = Dir3::new(transform.affine().transform_vector3(Vec3::X))
        .expect("rotation preserves a nonzero direction");
    let candidate = ChunkCandidate {
        chunk_reference: ChunkReference::new(entity, IVec3::ZERO),
        chunk: &local_coordinate.chunks[&IVec3::ZERO],
        transform,
    };

    let hit = raycast_chunk(
        candidate,
        Ray3d::new(world_origin, world_direction),
        RaySegment {
            virtual_coordinate: [0; 3],
            enter_distance: 0.0,
            exit_distance: 10.0,
        },
    )
    .expect("rotated chunk should be raycast in its local space");

    assert!((hit.distance - 2.0).abs() <= 1.0e-5);
    assert!(
        hit.point
            .abs_diff_eq(transform.transform_point(Vec3::new(0.0, 0.5, 0.5)), 1.0e-5)
    );
    assert!(hit.normal.abs_diff_eq(Vec3::NEG_Y, 1.0e-5));
}

#[test]
fn returns_the_nearest_result_after_parallel_chunk_checks_finish() {
    ComputeTaskPool::get_or_init(TaskPool::new);
    let mut world = bevy::prelude::World::new();
    let nearest_entity = world.spawn_empty().id();
    let farther_entity = world.spawn_empty().id();
    let nearest = LocalCoordinate::from_voxels([solid_voxel(IVec3::new(1, 0, 0))]);
    let farther = LocalCoordinate::from_voxels([solid_voxel(IVec3::new(4, 0, 0))]);
    let transform = GlobalTransform::default();
    let mut index = VirtualChunkIndex::default();
    index.insert(nearest_entity, IVec3::ZERO, &transform);
    index.insert(farther_entity, IVec3::ZERO, &transform);
    let coordinates = HashMap::from([
        (nearest_entity, (nearest, transform)),
        (farther_entity, (farther, transform)),
    ]);

    let hit = raycast_virtual_chunks(
        &index,
        Ray3d::new(Vec3::new(-1.0, 0.5, 0.5), Dir3::X),
        10.0,
        |chunk_reference| test_candidate(&coordinates, chunk_reference),
    )
    .expect("one of the parallel chunk checks should hit");

    assert_eq!(hit.chunk.local_coordinate_entity(), nearest_entity);
    assert_eq!(hit.distance, 2.0);
}

#[test]
fn respects_the_required_maximum_distance() {
    let mut world = bevy::prelude::World::new();
    let entity = world.spawn_empty().id();
    let local_coordinate = LocalCoordinate::from_voxels([solid_voxel(IVec3::new(4, 0, 0))]);
    let transform = GlobalTransform::default();
    let mut index = VirtualChunkIndex::default();
    index.insert(entity, IVec3::ZERO, &transform);
    let coordinates = HashMap::from([(entity, (local_coordinate, transform))]);

    let hit = raycast_virtual_chunks(
        &index,
        Ray3d::new(Vec3::new(-1.0, 0.5, 0.5), Dir3::X),
        4.0,
        |chunk_reference| test_candidate(&coordinates, chunk_reference),
    );

    assert!(hit.is_none());
}

#[test]
fn traverses_virtual_chunks_in_ray_order() {
    let coordinates =
        VirtualChunkTraversal::new(Ray3d::new(Vec3::new(1.0, 1.0, 1.0), Dir3::X), 40.0)
            .map(|segment| segment.virtual_coordinate)
            .collect::<Vec<_>>();

    assert_eq!(coordinates, vec![[0, 0, 0], [1, 0, 0], [2, 0, 0]]);
}

#[test]
fn traverses_the_negative_side_when_starting_on_a_boundary() {
    let coordinates =
        VirtualChunkTraversal::new(Ray3d::new(Vec3::new(16.0, 1.0, 1.0), Dir3::NEG_X), 20.0)
            .map(|segment| segment.virtual_coordinate)
            .collect::<Vec<_>>();

    assert_eq!(coordinates, vec![[0, 0, 0], [-1, 0, 0]]);
}
