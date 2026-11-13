//! Irradiance and incident-flux distributions over the studio rig.
use super::{
    super::{
        LightingModel, LightingPreset,
        rig::unit_radiance,
        sun::{SUN_ONE_MINUS_COS, SUN_SOLID_ANGLE, sun_radiance_factor},
        tray::{IRRADIANCE_SAMPLES, hemisphere_direction},
    },
    rng::SplitMix64,
};
use crate::optics::studio_rig::StudioRig;
use glam::Vec3;
use std::f64::consts::PI;

/// Discrete studio-light integral and its jittered inverse-CDF sampler.
pub(super) struct LightTable {
    dirs: Vec<Vec3>,
    cdf: Vec<f64>,
    /// Unnormalized irradiance or incident-flux integral.
    pub(super) total: f64,
    /// `1 - cos` of the cell cone's half-angle, kept exact rather than recovered from a cosine.
    cell_one_minus_cos: f32,
    /// The sun cell's direction and the exact `1 - cos` of its disc's half-angle.
    sun: Option<(Vec3, f32)>,
}

impl LightTable {
    /// Weights directions by radiance and their cosine to the tray.
    pub(super) fn gather(preset: LightingPreset, yaw: f32, pitch: f32) -> Self {
        Self::new(preset, yaw, pitch, true)
    }
    /// Weights photon directions by radiance alone.
    pub(super) fn emission(preset: LightingPreset, yaw: f32, pitch: f32) -> Self {
        Self::new(preset, yaw, pitch, false)
    }
    fn new(preset: LightingPreset, yaw: f32, pitch: f32, gather: bool) -> Self {
        let rig = StudioRig::new(yaw, pitch);
        let sun = (preset.model() == LightingModel::DaylightSun)
            .then_some((rig.key_dir, SUN_ONE_MINUS_COS));
        let geometry = if sun.is_some() {
            LightingPreset::DaylightDome
        } else {
            preset
        };
        let omega = 2.0 * PI / f64::from(IRRADIANCE_SAMPLES);
        let cells =
            usize::try_from(IRRADIANCE_SAMPLES).expect("hemisphere sample count fits usize");
        let mut dirs = Vec::with_capacity(cells + usize::from(sun.is_some()));
        let mut cdf = Vec::with_capacity(dirs.capacity());
        let mut total = 0.0;
        for i in 0..IRRADIANCE_SAMPLES {
            let (dir, cosine) = hemisphere_direction(i);
            total = (f64::from(unit_radiance(dir, geometry, &rig)) * omega)
                .mul_add(if gather { cosine } else { 1.0 }, total);
            dirs.push(dir);
            cdf.push(total);
        }
        if let Some((key, _)) = sun {
            total = (f64::from(sun_radiance_factor(key)) * f64::from(SUN_SOLID_ANGLE)).mul_add(
                if gather {
                    f64::from(key.y.max(0.0))
                } else {
                    1.0
                },
                total,
            );
            dirs.push(key);
            cdf.push(total);
        }
        if total > 0.0 {
            for c in &mut cdf {
                *c /= total;
            }
        }
        Self {
            dirs,
            cdf,
            total,
            cell_one_minus_cos: cell_cone_one_minus_cos(omega),
            sun,
        }
    }
    /// Picks a cell by its inclusive CDF and jitters uniformly within its cone.
    pub(super) fn sample(&self, u: f64, rng: &mut SplitMix64) -> Vec3 {
        let index = self
            .cdf
            .partition_point(|&c| c <= u)
            .min(self.dirs.len() - 1);
        let axis = self.dirs[index];
        let cone = if index
            == usize::try_from(IRRADIANCE_SAMPLES).expect("hemisphere sample count fits usize")
        {
            self.sun
                .map_or(self.cell_one_minus_cos, |(_, one_minus_cos)| one_minus_cos)
        } else {
            self.cell_one_minus_cos
        };
        // `1 - cos` stays exact, as in `sun_cone_direction`: a sun disc's cosine is within
        // an ulp of one, so `1 - cos_max` recovered from it would be mostly rounding.
        let one_minus_cos = rng.next_f32() * cone;
        let cos_t = 1.0 - one_minus_cos;
        let sin_t = (one_minus_cos * (2.0 - one_minus_cos)).max(0.0).sqrt();
        let phi = std::f32::consts::TAU * rng.next_f32();
        let (sin_phi, cos_phi) = phi.sin_cos();
        let (a, b) = axis.any_orthonormal_pair();
        axis * cos_t + (a * cos_phi + b * sin_phi) * sin_t
    }
}

/// `1 - cos(r)` of the cell cone's half-angle `r = sqrt(omega / pi)`, evaluated as
/// `2 sin²(r / 2)` in double precision and rounded once to the sampler's f32 format.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the cone angle is deliberately rounded to f32"
)]
fn cell_cone_one_minus_cos(omega: f64) -> f32 {
    let half_sine = (0.5 * (omega / PI).sqrt()).sin();
    (2.0 * half_sine * half_sine) as f32
}
