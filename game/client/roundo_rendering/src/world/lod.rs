//! Distance-based chunk LOD selection with asymmetric hysteresis.

use bevy::math::{Mat4, Vec3};

use super::{CHUNK_EDGE, MAX_CHUNK_LOD};

/// Multiplier applied only when crossing from a finer LOD to a coarser one.
pub(super) const LOD_COARSENING_HYSTERESIS_RATIO: f32 = 1.125;

/// Projected edge length at which the next coarser Cell becomes admissible.
///
/// Two pixels is the Nyquist-aligned threshold for the current hard-edged
/// procedural colors. It also moves geometry reduction closer to the camera
/// than the former one-pixel rule, reducing distant meshing and fragment detail.
const LOD_PROJECTED_CELL_TARGET_PIXELS: f32 = 2.0;

/// Computes squared world-distance thresholds for target-sized projected cells.
///
/// Index zero is the LOD 0→1 boundary; each later index doubles cell edge
/// length and therefore quadruples squared transition distance. The viewport
/// height is clamped to at least one pixel and projection scale is unsigned.
pub(super) fn lod_distance_thresholds_squared(
    viewport_height: f32,
    projection_scale_y: f32,
    effective_scale: f32,
) -> [f32; MAX_CHUNK_LOD as usize] {
    let focal_pixels = viewport_height.max(1.0) * projection_scale_y.abs() * 0.5;
    std::array::from_fn(|index| {
        let lod = index as u8 + 1;
        let cell_world_size = ((1_u32 << lod) as f32) * effective_scale;
        (cell_world_size * focal_pixels / LOD_PROJECTED_CELL_TARGET_PIXELS).powi(2)
    })
}

/// Snapshot of camera-chunk, axis scale, and thresholds shared by one view.
///
/// Selections remain comparable only while these values are unchanged.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LodContext {
    camera_chunk: Vec3,
    axis_scale: Vec3,
    thresholds_squared: [f32; MAX_CHUNK_LOD as usize],
}

/// Derives a view snapshot from a column-major local-to-world transform.
///
/// Camera position is transformed into local space and quantized down to a
/// chunk coordinate. Returns `None` when the conservative maximum axis scale is
/// non-finite or non-positive.
pub(super) fn context(
    world_from_local: [f32; 16],
    camera_position: Vec3,
    viewport_height: f32,
    projection_scale_y: f32,
) -> Option<LodContext> {
    let transform = Mat4::from_cols_array(&world_from_local);
    let axis_scale = Vec3::new(
        transform.x_axis.truncate().length(),
        transform.y_axis.truncate().length(),
        transform.z_axis.truncate().length(),
    );
    // The largest axis scale conservatively preserves detail under anisotropy.
    let effective_scale = axis_scale.max_element();
    if !effective_scale.is_finite() || effective_scale <= 0.0 {
        return None;
    }
    let local_camera = transform.inverse().transform_point3(camera_position);
    Some(LodContext {
        camera_chunk: (local_camera / CHUNK_EDGE as f32).floor(),
        axis_scale,
        thresholds_squared: lod_distance_thresholds_squared(
            viewport_height,
            projection_scale_y,
            effective_scale,
        ),
    })
}

/// Selects a bounded LOD using squared world distance on all three axes.
///
/// `None` selects directly from the theoretical thresholds. A previous level
/// is clamped to [`MAX_CHUNK_LOD`], retains detail through the coarsening
/// hysteresis band, and refines immediately after crossing a finer threshold.
pub(super) fn selected_lod(
    target_chunk: [i64; 3],
    context: &LodContext,
    current_lod: Option<u8>,
) -> u8 {
    let target_from_camera = Vec3::new(
        target_chunk[0] as f32 - context.camera_chunk.x,
        target_chunk[1] as f32 - context.camera_chunk.y,
        target_chunk[2] as f32 - context.camera_chunk.z,
    );
    // Distance is measured after per-axis world scaling.
    let physical_delta = target_from_camera * CHUNK_EDGE as f32 * context.axis_scale;
    let distance_squared = physical_delta.length_squared();
    let thresholds = context.thresholds_squared;

    // Initial selection has no hysteresis because no previous level exists.
    let Some(mut lod) = current_lod.map(|lod| lod.min(MAX_CHUNK_LOD)) else {
        let mut lod = 0;
        while lod < MAX_CHUNK_LOD && distance_squared >= thresholds[lod as usize] {
            lod += 1;
        }
        return lod;
    };

    // Squared thresholds require a squared hysteresis multiplier.
    let coarsening_multiplier = LOD_COARSENING_HYSTERESIS_RATIO.powi(2);
    while lod < MAX_CHUNK_LOD
        && distance_squared >= thresholds[lod as usize] * coarsening_multiplier
    {
        lod += 1;
    }
    while lod > 0 && distance_squared < thresholds[lod as usize - 1] {
        lod -= 1;
    }
    lod
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected_lod(
        world_from_chunk: [f32; 16],
        camera_position: Vec3,
        viewport_height: f32,
        projection_scale_y: f32,
        current_lod: Option<u8>,
    ) -> u8 {
        let chunk = Mat4::from_cols_array(&world_from_chunk);
        let axis_scale = Vec3::new(
            chunk.x_axis.truncate().length(),
            chunk.y_axis.truncate().length(),
            chunk.z_axis.truncate().length(),
        );
        let coordinate = (chunk.w_axis.truncate() / (axis_scale * CHUNK_EDGE as f32))
            .round()
            .as_ivec3();
        let world_from_local = Mat4::from_scale(axis_scale).to_cols_array();
        let context = context(
            world_from_local,
            camera_position,
            viewport_height,
            projection_scale_y,
        )
        .unwrap();
        super::selected_lod(
            [
                i64::from(coordinate.x),
                i64::from(coordinate.y),
                i64::from(coordinate.z),
            ],
            &context,
            current_lod,
        )
    }

    fn translated_chunk(coordinate: [f32; 3], scale: Vec3) -> [f32; 16] {
        let translation = Vec3::from_array(coordinate) * CHUNK_EDGE as f32 * scale;
        Mat4::from_scale_rotation_translation(scale, bevy::math::Quat::IDENTITY, translation)
            .to_cols_array()
    }

    #[test]
    fn thresholds_are_squared_and_double_in_distance_per_lod() {
        let thresholds = lod_distance_thresholds_squared(1080.0, 1.0, 1.0);
        assert_eq!(thresholds[0], (2.0 * 540.0 / 2.0_f32).powi(2));
        for pair in thresholds.windows(2) {
            assert_eq!(pair[1], pair[0] * 4.0);
        }
    }

    #[test]
    fn initial_selection_uses_the_theoretical_squared_boundaries() {
        let viewport_height = 32.0;
        let projection_scale = 2.0;
        assert_eq!(
            selected_lod(
                translated_chunk([1.0, 0.0, 0.0], Vec3::ONE),
                Vec3::ZERO,
                viewport_height,
                projection_scale,
                None,
            ),
            0
        );
        assert_eq!(
            selected_lod(
                translated_chunk([2.0, 0.0, 0.0], Vec3::ONE),
                Vec3::ZERO,
                viewport_height,
                projection_scale,
                None,
            ),
            1
        );
    }

    #[test]
    fn hysteresis_delays_coarsening_but_not_refinement() {
        let viewport_height = 32.0;
        let projection_scale = 2.0;
        assert_eq!(
            selected_lod(
                translated_chunk([2.0, 0.0, 0.0], Vec3::ONE),
                Vec3::ZERO,
                viewport_height,
                projection_scale,
                Some(0),
            ),
            0
        );
        assert_eq!(
            selected_lod(
                translated_chunk([3.0, 0.0, 0.0], Vec3::ONE),
                Vec3::ZERO,
                viewport_height,
                projection_scale,
                Some(0),
            ),
            1
        );
        assert_eq!(
            selected_lod(
                translated_chunk([1.0, 0.0, 0.0], Vec3::ONE),
                Vec3::ZERO,
                viewport_height,
                projection_scale,
                Some(1),
            ),
            0
        );
    }

    #[test]
    fn uniform_scale_cancels_between_distance_and_cell_size() {
        let unscaled = selected_lod(
            translated_chunk([32.0, 4.0, 0.0], Vec3::ONE),
            Vec3::ZERO,
            1080.0,
            1.0,
            None,
        );
        let scaled = selected_lod(
            translated_chunk([32.0, 4.0, 0.0], Vec3::splat(3.0)),
            Vec3::ZERO,
            1080.0,
            1.0,
            None,
        );
        assert_eq!(scaled, unscaled);
    }

    #[test]
    fn distance_uses_all_three_scaled_axes_without_square_root() {
        let x = selected_lod(
            translated_chunk([24.0, 0.0, 0.0], Vec3::new(2.0, 1.0, 1.0)),
            Vec3::ZERO,
            256.0,
            1.0,
            None,
        );
        let y = selected_lod(
            translated_chunk([0.0, 24.0, 0.0], Vec3::new(2.0, 1.0, 1.0)),
            Vec3::ZERO,
            256.0,
            1.0,
            None,
        );
        assert!(x > y);
    }
}
