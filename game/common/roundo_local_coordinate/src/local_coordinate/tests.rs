use super::data::{
    AtomicVoxel, EMPTY_VOXEL_ID, GLOBAL_ATOMIC_VOXEL_DATA, LocalCoordinate, PositionedAtomicVoxel,
    SOLID_VOXEL_ID,
};
use bevy::prelude::{IVec3, Vec3};

#[test]
fn atomic_voxels_are_compact_identifiers_with_global_definitions() {
    assert_eq!(
        std::mem::size_of::<AtomicVoxel>(),
        std::mem::size_of::<u32>()
    );
    assert_eq!(EMPTY_VOXEL_ID.0, 0);
    assert_eq!(SOLID_VOXEL_ID.0, 1);
    assert!(GLOBAL_ATOMIC_VOXEL_DATA.contains_key(&EMPTY_VOXEL_ID));
    assert!(GLOBAL_ATOMIC_VOXEL_DATA.contains_key(&SOLID_VOXEL_ID));
}

fn solid_voxel(position: IVec3) -> PositionedAtomicVoxel {
    PositionedAtomicVoxel {
        position,
        voxel: SOLID_VOXEL_ID,
    }
}

#[test]
fn content_revisions_only_advance_for_authoritative_changes() {
    let mut local_coordinate = LocalCoordinate::default();
    assert!(local_coordinate.apply_voxel(solid_voxel(IVec3::ZERO)));
    let coordinate_revision = local_coordinate.content_revision;
    let chunk_revision = local_coordinate.chunks[&IVec3::ZERO].content_revision;

    assert!(!local_coordinate.apply_voxel(solid_voxel(IVec3::ZERO)));
    assert_eq!(local_coordinate.content_revision, coordinate_revision);
    assert_eq!(
        local_coordinate.chunks[&IVec3::ZERO].content_revision,
        chunk_revision
    );

    assert!(local_coordinate.apply_voxel(solid_voxel(IVec3::X)));
    assert!(local_coordinate.content_revision > coordinate_revision);
    assert!(local_coordinate.chunks[&IVec3::ZERO].content_revision > chunk_revision);
}

#[test]
fn center_of_mass_uses_chunk_fill_as_weight() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(0, 0, 0)),
        solid_voxel(IVec3::new(16, 0, 0)),
        solid_voxel(IVec3::new(17, 0, 0)),
        solid_voxel(IVec3::new(18, 0, 0)),
    ]);

    assert_eq!(local_coordinate.center_of_mass, Vec3::new(20.0, 8.0, 8.0));

    local_coordinate.apply_voxel(PositionedAtomicVoxel {
        position: IVec3::new(0, 0, 0),
        voxel: EMPTY_VOXEL_ID,
    });
    assert_eq!(local_coordinate.center_of_mass, Vec3::new(24.0, 8.0, 8.0));
}
