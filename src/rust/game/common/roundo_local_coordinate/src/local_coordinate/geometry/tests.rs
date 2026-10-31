use super::*;
use crate::SOLID_VOXEL_ID;

fn assert_full_scan_equivalent(coordinate: &LocalCoordinate) {
    let mut moments = [0.0; 3];
    let mut count = 0u64;
    for (position, chunk) in &coordinate.chunks {
        let center = (position.as_dvec3() + 0.5) * f64::from(CHUNK_EDGE_LENGTH);
        for (moment, value) in moments.iter_mut().zip(center.to_array()) {
            *moment += value * chunk.solid_count as f64;
        }
        count += chunk.solid_count as u64;
    }
    let expected = if count == 0 {
        Vec3::ZERO
    } else {
        Vec3::from_array(moments.map(|moment| (moment / count as f64) as f32))
    };
    assert_eq!(coordinate.total_solid_voxels, count);
    assert_eq!(coordinate.center_of_mass, expected);
}

#[test]
fn incremental_mass_matches_full_scan_across_edits_installations_and_evictions() {
    let mut coordinate = LocalCoordinate::default();
    let mut seed = 17u64;
    for step in 0..1500 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let position = IVec3::new(
            (seed % 11) as i32 - 5,
            ((seed >> 8) % 5) as i32 - 2,
            ((seed >> 16) % 7) as i32 - 3,
        );
        let voxel_position = position * CHUNK_EDGE_LENGTH;
        match step % 6 {
            0 => {
                coordinate.apply_voxel(PositionedAtomicVoxel {
                    position: voxel_position,
                    voxel: SOLID_VOXEL_ID,
                });
            }
            1 => {
                coordinate.apply_voxels([
                    PositionedAtomicVoxel {
                        position: voxel_position + IVec3::X,
                        voxel: SOLID_VOXEL_ID,
                    },
                    PositionedAtomicVoxel {
                        position: voxel_position + IVec3::Y,
                        voxel: SOLID_VOXEL_ID,
                    },
                ]);
            }
            2 => {
                let mut chunk = Chunk::default();
                for x in 0..(seed % 16) as i32 {
                    chunk.set_voxel(IVec3::new(x, 0, 0), SOLID_VOXEL_ID);
                }
                coordinate.replace_chunk(position, chunk);
            }
            3 => {
                coordinate.apply_voxel(PositionedAtomicVoxel {
                    position: voxel_position,
                    voxel: EMPTY_VOXEL_ID,
                });
            }
            4 => {
                coordinate.remove_chunk(position);
            }
            _ => {
                coordinate.mark_chunk_loaded(position);
            }
        }
        assert_full_scan_equivalent(&coordinate);
    }
    let positions = coordinate.chunks.keys().copied().collect::<Vec<_>>();
    for position in positions {
        coordinate.remove_chunk(position);
        assert_full_scan_equivalent(&coordinate);
    }
    assert_eq!(coordinate.center_of_mass, Vec3::ZERO);
    assert_eq!(coordinate.weighted_chunk_centers, [0.0; 3]);
}
