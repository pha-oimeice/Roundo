use super::data::{
    AtomicVoxel, AtomicVoxelData, CHUNK_EDGE_LENGTH, EMPTY_VOXEL_ID, LocalCoordinate,
    SOLID_VOXEL_ID,
};
use bevy::prelude::{IVec3, Vec3};

fn solid_voxel(position: IVec3) -> AtomicVoxel {
    AtomicVoxel {
        position,
        data: AtomicVoxelData { id: SOLID_VOXEL_ID },
    }
}

const CHUNK_NEIGHBOR_OFFSETS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

fn load_complete_neighborhoods(
    local_coordinate: &mut LocalCoordinate,
    chunk_positions: impl IntoIterator<Item = IVec3>,
) {
    for chunk_position in chunk_positions {
        local_coordinate.mark_chunk_loaded(chunk_position);
        for offset in CHUNK_NEIGHBOR_OFFSETS {
            local_coordinate.mark_chunk_loaded(chunk_position + offset);
        }
    }
}

fn triangle_count(local_coordinate: &LocalCoordinate) -> usize {
    local_coordinate
        .chunks
        .values()
        .map(|chunk| chunk.triangles.len())
        .sum()
}

#[test]
fn culls_the_internal_face_between_voxels_in_one_chunk() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(0, 0, 0)),
        solid_voxel(IVec3::new(1, 0, 0)),
    ]);
    load_complete_neighborhoods(&mut local_coordinate, [IVec3::ZERO]);

    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(triangle_count(&local_coordinate), 20);
}

#[test]
fn culls_the_internal_face_between_adjacent_chunks() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(15, 0, 0)),
        solid_voxel(IVec3::new(16, 0, 0)),
    ]);
    load_complete_neighborhoods(&mut local_coordinate, [IVec3::ZERO, IVec3::X]);

    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(triangle_count(&local_coordinate), 20);
}

#[test]
fn updates_only_the_touching_chunk_when_a_boundary_voxel_is_removed() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::new(15, 0, 0)),
        solid_voxel(IVec3::new(16, 0, 0)),
        solid_voxel(IVec3::new(32, 0, 0)),
    ]);
    load_complete_neighborhoods(
        &mut local_coordinate,
        [IVec3::ZERO, IVec3::X, IVec3::new(2, 0, 0)],
    );
    local_coordinate.rebuild_dirty_chunks();

    assert!(local_coordinate.apply_voxel(AtomicVoxel {
        position: IVec3::new(15, 0, 0),
        data: AtomicVoxelData { id: EMPTY_VOXEL_ID },
    }));
    assert_eq!(local_coordinate.dirty_chunks.len(), 2);
    assert!(!local_coordinate.dirty_chunks.contains(&IVec3::new(2, 0, 0)));
    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(triangle_count(&local_coordinate), 24);
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

    local_coordinate.apply_voxel(AtomicVoxel {
        position: IVec3::new(0, 0, 0),
        data: AtomicVoxelData { id: EMPTY_VOXEL_ID },
    });
    assert_eq!(local_coordinate.center_of_mass, Vec3::new(24.0, 8.0, 8.0));
}

#[test]
fn does_not_build_an_intermediate_mesh_without_all_neighbors() {
    let mut local_coordinate = LocalCoordinate::default();
    local_coordinate.apply_voxel(solid_voxel(IVec3::ZERO));

    assert!(local_coordinate.rebuild_dirty_chunks());
    assert!(local_coordinate.chunks[&IVec3::ZERO].triangles.is_empty());

    load_complete_neighborhoods(&mut local_coordinate, [IVec3::ZERO]);
    assert!(local_coordinate.rebuild_dirty_chunks());
    assert_eq!(local_coordinate.chunks[&IVec3::ZERO].triangles.len(), 12);
}

#[test]
fn fully_enclosed_solid_chunk_has_no_mesh() {
    let mut voxels = Vec::new();
    for z in 0..CHUNK_EDGE_LENGTH {
        for y in 0..CHUNK_EDGE_LENGTH {
            for x in 0..CHUNK_EDGE_LENGTH {
                voxels.push(solid_voxel(IVec3::new(x, y, z)));
            }
        }
    }
    for first in 0..CHUNK_EDGE_LENGTH {
        for second in 0..CHUNK_EDGE_LENGTH {
            voxels.extend([
                solid_voxel(IVec3::new(-1, first, second)),
                solid_voxel(IVec3::new(CHUNK_EDGE_LENGTH, first, second)),
                solid_voxel(IVec3::new(first, -1, second)),
                solid_voxel(IVec3::new(first, CHUNK_EDGE_LENGTH, second)),
                solid_voxel(IVec3::new(first, second, -1)),
                solid_voxel(IVec3::new(first, second, CHUNK_EDGE_LENGTH)),
            ]);
        }
    }
    let mut local_coordinate = LocalCoordinate::from_voxels(voxels);

    assert!(local_coordinate.rebuild_dirty_chunks());
    assert!(local_coordinate.chunks[&IVec3::ZERO].triangles.is_empty());
}

#[test]
fn modifying_one_chunk_preserves_other_chunk_geometry() {
    let mut local_coordinate = LocalCoordinate::from_voxels([
        solid_voxel(IVec3::ZERO),
        solid_voxel(IVec3::new(CHUNK_EDGE_LENGTH * 2, 0, 0)),
    ]);
    assert!(local_coordinate.rebuild_dirty_chunks());
    let untouched_position = IVec3::new(2, 0, 0);
    let untouched_revision = local_coordinate.chunks[&untouched_position].geometry_revision;

    assert!(local_coordinate.apply_voxel(solid_voxel(IVec3::new(1, 1, 1))));
    assert!(local_coordinate.rebuild_dirty_chunks());

    assert_eq!(
        local_coordinate.chunks[&untouched_position].geometry_revision,
        untouched_revision
    );
    assert!(local_coordinate.chunks[&IVec3::ZERO].geometry_revision > untouched_revision);
}

#[test]
fn chunk_triangles_use_chunk_local_vertices() {
    let chunk_position = IVec3::new(2, -1, 3);
    let voxel_position = chunk_position * CHUNK_EDGE_LENGTH;
    let mut local_coordinate = LocalCoordinate::from_voxels([solid_voxel(voxel_position)]);
    assert!(local_coordinate.rebuild_dirty_chunks());

    for triangle in &local_coordinate.chunks[&chunk_position].triangles {
        for vertex in triangle.vertices {
            assert!((0.0..=1.0).contains(&vertex.x));
            assert!((0.0..=1.0).contains(&vertex.y));
            assert!((0.0..=1.0).contains(&vertex.z));
        }
    }
}
