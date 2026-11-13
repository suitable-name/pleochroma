//! Plane intersection and interpolation, with arithmetic mirrored by the GPU lookup.

use super::{
    TRAY_SHADOW_BAND_WIDTH_NM, TRAY_SHADOW_BANDS, TRAY_SHADOW_FIRST_CENTER_NM, TrayShadowMap,
};
use glam::Vec3;

/// Index of the last band: the upper clamp and neighbour bound of the band coordinate.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the band count is a small compile-time constant"
)]
const LAST_BAND: u32 = (TRAY_SHADOW_BANDS - 1) as u32;
/// [`LAST_BAND`] as the float the band coordinate is clamped to.
const LAST_BAND_F: f32 = (TRAY_SHADOW_BANDS - 1) as f32;

/// Linear interpolation in the operation order shared with WGSL.
const fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Neighbour indices and interpolation weight for a clamped texel coordinate.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the coordinate is clamped to the nonnegative u32 map dimension"
)]
fn neighbours(value: f32, last: u32) -> (usize, usize, f32) {
    let lower = value.floor() as u32;
    let upper = (lower + 1).min(last);
    (lower as usize, upper as usize, value - lower as f32)
}

impl TrayShadowMap {
    /// Where a downward ray from above the tray meets it, in map coordinates `[-1, 1]²`.
    /// The negated comparisons send a NaN component to the no-shadow path, as the WGSL twin does.
    #[must_use]
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "C7 and the WGSL state the tests negated so that NaN means no shadow"
    )]
    #[expect(
        clippy::suboptimal_flops,
        reason = "C7: plain operations mirrored in WGSL; mul_add would break the CPU/GPU twin"
    )]
    pub fn plane_uv(&self, origin: Vec3, dir: Vec3) -> Option<[f32; 2]> {
        if !(dir.y < 0.0) || !(origin.y > self.plane_y) {
            return None;
        }
        let travel = (self.plane_y - origin.y) / dir.y;
        let hit_x = origin.x + dir.x * travel;
        let hit_z = origin.z + dir.z * travel;
        let map_u = (hit_x - self.center[0]) / self.half_extent;
        let map_v = (hit_z - self.center[1]) / self.half_extent;
        if !(map_u.abs() <= 1.0) || !(map_v.abs() <= 1.0) {
            return None;
        }
        Some([map_u, map_v])
    }

    /// Spatial interpolation indices and weights, clamped to the outer texel centres.
    #[expect(
        clippy::suboptimal_flops,
        reason = "C7: plain operations mirrored in WGSL; mul_add would break the CPU/GPU twin"
    )]
    fn spatial(&self, uv: [f32; 2]) -> ((usize, usize, f32), (usize, usize, f32)) {
        let half = 0.5 * self.resolution as f32;
        let last = self.resolution - 1;
        let fx = ((uv[0] + 1.0) * half - 0.5).clamp(0.0, last as f32);
        let fy = ((uv[1] + 1.0) * half - 0.5).clamp(0.0, last as f32);
        (neighbours(fx, last), neighbours(fy, last))
    }

    /// Irradiance ratio at map coordinates and wavelength, interpolated over centres.
    /// Coordinates outside the map give exactly one, leaving the tray unchanged.
    #[must_use]
    pub fn factor_at(&self, uv: [f32; 2], lambda_nm: f32) -> f32 {
        if !uv.iter().all(|v| v.abs() <= 1.0) {
            return 1.0;
        }
        let ((i0, i1, ax), (j0, j1, ay)) = self.spatial(uv);
        let fb = ((lambda_nm - TRAY_SHADOW_FIRST_CENTER_NM) / TRAY_SHADOW_BAND_WIDTH_NM)
            .clamp(0.0, LAST_BAND_F);
        let (b0, b1, ab) = neighbours(fb, LAST_BAND);
        let n = self.resolution as usize;
        let bil = |b| {
            let texel = |i: usize, j: usize| self.factors[(j * n + i) * TRAY_SHADOW_BANDS + b];
            lerp(
                lerp(texel(i0, j0), texel(i1, j0), ax),
                lerp(texel(i0, j1), texel(i1, j1), ax),
                ay,
            )
        };
        lerp(bil(b0), bil(b1), ab)
    }

    /// Every channel's ratio, or `None` when the ray does not meet this map.
    #[must_use]
    pub fn factors_for<const N: usize>(
        &self,
        origin: Vec3,
        dir: Vec3,
        lambdas: &[f32; N],
    ) -> Option<[f32; N]> {
        let uv = self.plane_uv(origin, dir)?;
        Some(std::array::from_fn(|k| self.factor_at(uv, lambdas[k])))
    }

    /// Bilinear linear-sRGB ratios for a Ray-Traced ray meeting this map.
    #[must_use]
    pub fn factor_rgb_for(&self, origin: Vec3, dir: Vec3) -> Option<[f32; 3]> {
        let uv = self.plane_uv(origin, dir)?;
        let ((i0, i1, ax), (j0, j1, ay)) = self.spatial(uv);
        let n = self.resolution as usize;
        Some(std::array::from_fn(|k| {
            let texel = |i: usize, j: usize| self.factors_rgb[j * n + i][k];
            lerp(
                lerp(texel(i0, j0), texel(i1, j0), ax),
                lerp(texel(i0, j1), texel(i1, j1), ax),
                ay,
            )
        }))
    }
}
