use super::data::{AtomicVoxel, AtomicVoxelData, EMPTY_VOXEL_ID, LocalCoordinate, SOLID_VOXEL_ID};
use bevy::prelude::IVec3;

fn solid_voxel(position: IVec3) -> AtomicVoxel {
    AtomicVoxel {
        position,
        data: AtomicVoxelData { id: SOLID_VOXEL_ID },
    }
}

#[test]
fn culls_the_internal_face_between_voxels_in_one_chunk() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(0, 0, 0)),
        solid_voxel(IVec3::new(1, 0, 0)),
    ]);

    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(local_coordinate.triangles.len(), 20);
}

#[test]
fn culls_the_internal_face_between_adjacent_chunks() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(15, 0, 0)),
        solid_voxel(IVec3::new(16, 0, 0)),
    ]);

    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(local_coordinate.triangles.len(), 20);
}

#[test]
fn updates_only_the_touching_chunk_when_a_boundary_voxel_is_removed() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(15, 0, 0)),
        solid_voxel(IVec3::new(16, 0, 0)),
        solid_voxel(IVec3::new(32, 0, 0)),
    ]);
    local_coordinate.rebuild_dirty_chunks();

    assert!(local_coordinate.apply_voxel(AtomicVoxel {
        position: IVec3::new(15, 0, 0),
        data: AtomicVoxelData { id: EMPTY_VOXEL_ID },
    }));
    assert_eq!(local_coordinate.dirty_chunks.len(), 2);
    assert!(!local_coordinate.dirty_chunks.contains(&IVec3::new(2, 0, 0)));
    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(local_coordinate.triangles.len(), 24);
}
