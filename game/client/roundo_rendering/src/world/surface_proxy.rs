//! Adaptive-dual-contouring surface-proxy primitives.
//!
//! This module is the CPU reference and ABI definition for the staged GPU
//! implementation. QEF accumulation is associative, so one GPU workgroup can
//! initialize each finest SVO cell and each shallower depth can independently
//! reduce fixed groups of eight children. QEF residual is only a fitting metric;
//! topology and conservative surface-error checks must additionally approve a
//! proxy before rendering may select it.

#![allow(dead_code)]

use bevy::{math::DVec3, render::render_resource::ShaderType};

/// Symmetric quadratic-error accumulator for Hermite surface planes.
///
/// `ata` stores xx, xy, xz, yy, yz, zz. A sample `(p, n)` contributes the
/// plane equation `n·x = n·p`. Accumulators may be merged in any tree order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct QefAccumulator {
    ata: [f64; 6],
    atb: DVec3,
    btb: f64,
    mass_sum: DVec3,
    sample_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct QefSolution {
    pub(super) position: DVec3,
    /// Mean squared plane residual. This is not a conservative Hausdorff bound.
    pub(super) mean_squared_residual: f64,
}

impl QefAccumulator {
    pub(super) fn add_plane(&mut self, position: DVec3, normal: DVec3) {
        let length_squared = normal.length_squared();
        if !position.is_finite() || !normal.is_finite() || length_squared <= f64::EPSILON {
            return;
        }
        let normal = normal / length_squared.sqrt();
        let distance = normal.dot(position);
        self.ata[0] += normal.x * normal.x;
        self.ata[1] += normal.x * normal.y;
        self.ata[2] += normal.x * normal.z;
        self.ata[3] += normal.y * normal.y;
        self.ata[4] += normal.y * normal.z;
        self.ata[5] += normal.z * normal.z;
        self.atb += normal * distance;
        self.btb += distance * distance;
        self.mass_sum += position;
        self.sample_count = self.sample_count.saturating_add(1);
    }

    pub(super) fn merge(&mut self, other: Self) {
        for (left, right) in self.ata.iter_mut().zip(other.ata) {
            *left += right;
        }
        self.atb += other.atb;
        self.btb += other.btb;
        self.mass_sum += other.mass_sum;
        self.sample_count = self.sample_count.saturating_add(other.sample_count);
    }

    pub(super) const fn sample_count(self) -> u32 {
        self.sample_count
    }

    pub(super) fn solve(self, minimum: DVec3, maximum: DVec3) -> Option<QefSolution> {
        if self.sample_count == 0 || !minimum.is_finite() || !maximum.is_finite() {
            return None;
        }
        let mass_point = self.mass_sum / f64::from(self.sample_count);
        let trace = self.ata[0] + self.ata[3] + self.ata[5];
        let regularization = trace.max(1.0) * 1.0e-9;
        let matrix = [
            [self.ata[0] + regularization, self.ata[1], self.ata[2]],
            [self.ata[1], self.ata[3] + regularization, self.ata[4]],
            [self.ata[2], self.ata[4], self.ata[5] + regularization],
        ];
        let right = self.atb + mass_point * regularization;
        let unconstrained = solve_3x3(matrix, right).unwrap_or(mass_point);
        let position = unconstrained.clamp(minimum, maximum);
        let residual = self.evaluate(position).max(0.0) / f64::from(self.sample_count);
        Some(QefSolution {
            position,
            mean_squared_residual: residual,
        })
    }

    fn evaluate(self, position: DVec3) -> f64 {
        let quadratic = self.ata[0] * position.x * position.x
            + 2.0 * self.ata[1] * position.x * position.y
            + 2.0 * self.ata[2] * position.x * position.z
            + self.ata[3] * position.y * position.y
            + 2.0 * self.ata[4] * position.y * position.z
            + self.ata[5] * position.z * position.z;
        quadratic - 2.0 * self.atb.dot(position) + self.btb
    }
}

fn solve_3x3(matrix: [[f64; 3]; 3], right: DVec3) -> Option<DVec3> {
    let [[a, b, c], [d, e, f], [g, h, i]] = matrix;
    let determinant = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if !determinant.is_finite() || determinant.abs() <= f64::EPSILON {
        return None;
    }
    let inverse = 1.0 / determinant;
    let x = (right.x * (e * i - f * h) - b * (right.y * i - f * right.z)
        + c * (right.y * h - e * right.z))
        * inverse;
    let y = (a * (right.y * i - f * right.z) - right.x * (d * i - f * g)
        + c * (d * right.z - right.y * g))
        * inverse;
    let z = (a * (e * right.z - right.y * h) - b * (d * right.z - right.y * g)
        + right.x * (d * h - e * g))
        * inverse;
    let solution = DVec3::new(x, y, z);
    solution.is_finite().then_some(solution)
}

/// Stable 64-byte storage representation shared with the future WGSL reducer.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub(super) struct GpuQefSummary {
    /// xx, xy, xz, yy
    pub(super) ata0: bevy::math::Vec4,
    /// yz, zz, atb.x, atb.y
    pub(super) ata1: bevy::math::Vec4,
    /// atb.z, btb, mass_sum.x, mass_sum.y
    pub(super) qef_data: bevy::math::Vec4,
    /// mass_sum.z, sample_count, conservative_error, topology_flags
    pub(super) proxy_data: bevy::math::Vec4,
}

/// One depth-to-parent reduction over transient per-request QEF summaries.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub(super) struct GpuQefReducePass {
    pub(super) source_offset: u32,
    pub(super) target_offset: u32,
    pub(super) source_edge: u32,
    pub(super) target_count: u32,
    pub(super) summaries_per_request: u32,
    pub(super) request_count: u32,
    pub(super) reserved0: u32,
    pub(super) reserved1: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_resource::ShaderSize;

    fn box_planes(minimum: DVec3, maximum: DVec3) -> QefAccumulator {
        let mut qef = QefAccumulator::default();
        qef.add_plane(DVec3::new(minimum.x, 0.0, 0.0), DVec3::NEG_X);
        qef.add_plane(DVec3::new(maximum.x, 0.0, 0.0), DVec3::X);
        qef.add_plane(DVec3::new(0.0, minimum.y, 0.0), DVec3::NEG_Y);
        qef.add_plane(DVec3::new(0.0, maximum.y, 0.0), DVec3::Y);
        qef.add_plane(DVec3::new(0.0, 0.0, minimum.z), DVec3::NEG_Z);
        qef.add_plane(DVec3::new(0.0, 0.0, maximum.z), DVec3::Z);
        qef
    }

    #[test]
    fn child_qefs_merge_to_the_same_parent_solution() {
        let mut left = QefAccumulator::default();
        left.add_plane(DVec3::new(1.0, 0.0, 0.0), DVec3::X);
        left.add_plane(DVec3::new(0.0, 2.0, 0.0), DVec3::Y);
        let mut right = QefAccumulator::default();
        right.add_plane(DVec3::new(0.0, 0.0, 3.0), DVec3::Z);

        let mut merged = left;
        merged.merge(right);
        let mut direct = QefAccumulator::default();
        direct.add_plane(DVec3::new(1.0, 0.0, 0.0), DVec3::X);
        direct.add_plane(DVec3::new(0.0, 2.0, 0.0), DVec3::Y);
        direct.add_plane(DVec3::new(0.0, 0.0, 3.0), DVec3::Z);

        assert_eq!(merged, direct);
        let bounds = (DVec3::ZERO, DVec3::splat(4.0));
        let merged_solution = merged.solve(bounds.0, bounds.1).unwrap();
        let direct_solution = direct.solve(bounds.0, bounds.1).unwrap();
        assert!(
            merged_solution
                .position
                .abs_diff_eq(direct_solution.position, 1.0e-9)
        );
        assert!(
            merged_solution
                .position
                .abs_diff_eq(DVec3::new(1.0, 2.0, 3.0), 1.0e-8)
        );
    }

    #[test]
    fn enclosed_cavity_planes_are_not_treated_as_a_height_field() {
        let qef = box_planes(DVec3::new(2.0, 3.0, 4.0), DVec3::new(6.0, 9.0, 12.0));
        let solution = qef.solve(DVec3::ZERO, DVec3::splat(16.0)).unwrap();

        assert_eq!(qef.sample_count(), 6);
        assert!(
            solution
                .position
                .abs_diff_eq(DVec3::new(4.0, 6.0, 8.0), 1.0e-7),
            "unexpected enclosed-surface solution: {:?}",
            solution.position
        );
    }

    #[test]
    fn solution_is_clamped_to_its_octree_node_bounds() {
        let mut qef = QefAccumulator::default();
        qef.add_plane(DVec3::new(20.0, 0.0, 0.0), DVec3::X);
        let solution = qef.solve(DVec3::ZERO, DVec3::splat(4.0)).unwrap();
        assert_eq!(solution.position.x, 4.0);
    }

    #[test]
    fn gpu_qef_summary_keeps_the_documented_stride() {
        assert_eq!(GpuQefSummary::SHADER_SIZE.get(), 64);
        assert_eq!(GpuQefReducePass::SHADER_SIZE.get(), 32);
    }

    #[test]
    fn gpu_reducer_has_one_parallel_invocation_per_parent_and_request() {
        let shader = include_str!("shaders/world_surface_proxy.wgsl");
        assert!(shader.contains("@compute @workgroup_size(64)"));
        assert!(shader.contains("let request_index = invocation.y"));
        assert!(shader.contains("for (var octant = 0u; octant < 8u"));
        assert!(shader.contains("parent = merged(parent, summaries[child_index])"));
    }
}
