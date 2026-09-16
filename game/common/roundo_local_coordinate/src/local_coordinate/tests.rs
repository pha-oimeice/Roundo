// Core invariants for voxel identity, revisions, and aggregate mass.
use super::data::{
    AtomicVoxel, EMPTY_VOXEL_ID, LocalCoordinate, PositionedAtomicVoxel, SOLID_VOXEL_ID,
};
use super::voxel_registry::AtomicVoxelRegistry;
use bevy::prelude::{IVec3, Vec3};

#[test]
// Wire voxel values must remain compact and registry-backed.
fn atomic_voxels_are_compact_identifiers_with_resource_definitions() {
    assert_eq!(
        std::mem::size_of::<AtomicVoxel>(),
        std::mem::size_of::<u32>()
    );
    assert_eq!(EMPTY_VOXEL_ID.0, 0);
    assert_eq!(SOLID_VOXEL_ID.0, 1);
    let registry = AtomicVoxelRegistry::builtin();
    assert!(registry.definition(EMPTY_VOXEL_ID).is_none());
    assert_eq!(
        registry
            .definition(SOLID_VOXEL_ID)
            .unwrap()
            .name
            .to_string(),
        "vanilla.base.stone"
    );
}

fn solid_voxel(position: IVec3) -> PositionedAtomicVoxel {
    PositionedAtomicVoxel {
        position,
        voxel: SOLID_VOXEL_ID,
    }
}

#[test]
// Idempotent writes must not invalidate derived chunk state.
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
// Chunk centers are weighted by their occupied voxel counts.
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
