//! CPU statistics helpers shared by the equivalence tests of the path cache and the beam tracer.
//!
//! The helpers compare two streams of XYZ values by the z score of the difference of their means.
//! This module does not depend on the `gpu` feature: [`SIGMA_THRESHOLD`] and
//! [`NULL_OVER_3_SIGMA_RATE`] are copies of the constants in `renderer/gpu/z_stats.rs`, pinned
//! against them when that feature is on.

use glam::Vec3;

use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
    materials::GemMaterial,
    raytracer::{
        BACKDROP_GREY, EnvironmentSource, LightingPreset, cranley_patterson_rotate, hash_u32,
        radical_inverse_base,
    },
};

/// A z score above this size counts as a failure (copy of `gpu::z_stats::SIGMA_THRESHOLD`).
pub const SIGMA_THRESHOLD: f64 = 3.0;

/// The Gaussian rate of `|z| > 3` under the null hypothesis (copy of
/// `gpu::z_stats::NULL_OVER_3_SIGMA_RATE`).
pub const NULL_OVER_3_SIGMA_RATE: f64 = 0.0027;

/// The seed and stratified hero draw of sample `i` of a test stream.
#[must_use]
pub fn draw(i: u32, salt: u32) -> (u32, f32) {
    let seed = hash_u32(i.wrapping_mul(0x9E37_79B9) ^ salt);
    let offset = hash_u32(salt) as f32 / 4_294_967_295.0;
    (
        seed,
        cranley_patterson_rotate(radical_inverse_base(i, 5), offset),
    )
}

/// Running mean and variance of a stream of XYZ values.
#[derive(Default)]
pub struct Stats {
    n: f64,
    sum: [f64; 3],
    squares: [f64; 3],
}

impl Stats {
    /// Adds one value to the stream.
    pub fn add(&mut self, value: Vec3) {
        self.n += 1.0;
        for (i, v) in value.to_array().into_iter().enumerate() {
            let v = f64::from(v);
            self.sum[i] += v;
            self.squares[i] = v.mul_add(v, self.squares[i]);
        }
    }

    /// The mean of component `i`.
    #[must_use]
    pub fn mean(&self, i: usize) -> f64 {
        self.sum[i] / self.n
    }

    fn variance_of_mean(&self, i: usize) -> f64 {
        let mean = self.mean(i);
        let variance =
            mean.mul_add(-mean, self.squares[i] / self.n).max(0.0) * self.n / (self.n - 1.0);
        variance / self.n
    }

    /// The z score of the difference of two means, `None` when neither stream varies.
    #[must_use]
    pub fn z_against(&self, other: &Self, i: usize) -> Option<f64> {
        let spread = (self.variance_of_mean(i) + other.variance_of_mean(i)).sqrt();
        (spread > 0.0).then(|| (self.mean(i) - other.mean(i)) / spread)
    }
}

/// Every z at most `limit` in size, and the fraction above 3 within twice the Gaussian rate (plus
/// one value of slack for a short list).
///
/// # Panics
///
/// When `zs` is empty, when any `|z|` reaches `limit`, or when too many values exceed 3.
pub fn assert_z_stats(zs: &[f64], limit: f64, label: &str) {
    assert!(!zs.is_empty(), "{label}: nothing was compared");
    let worst = zs.iter().fold(0.0_f64, |a, z| a.max(z.abs()));
    assert!(
        worst < limit,
        "{label}: |z| = {worst:.2} (limit {limit}), all: {zs:?}"
    );
    let above_three = zs.iter().filter(|z| z.abs() > SIGMA_THRESHOLD).count();
    // `ceil(2 * 0.0027 * n)` in integers: twice `NULL_OVER_3_SIGMA_RATE`, in units of 1e-4.
    let allowed = (zs.len() * 54).div_ceil(10_000) + 1;
    assert!(
        above_three <= allowed,
        "{label}: {above_three} of {} values above 3 sigma (allowed {allowed}): {zs:?}",
        zs.len()
    );
}

/// A smooth studio environment without small lights, for stable statistics.
#[must_use]
pub fn smooth_environment() -> EnvironmentSource<'static> {
    LightingPreset::IsoHemisphere
        .studio(1.0, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY)
}

/// The sapphire of the benchmark in `benchmarks/render_accel`.
///
/// The built-in Sapphire with that job's band strengths and absorption path scale. Its ordinary
/// band at 585 nm lies between two nodes of the path cache's 50 nm comb.
#[must_use]
pub fn bench_sapphire() -> GemMaterial {
    let band = |center_nm: f32, width_nm: f32, peak: f32| AbsorptionBand {
        center_nm,
        width_nm,
        peak,
        shape: BandShape::GaussianEnergy,
    };
    let mut material = GemMaterial::sapphire();
    material.absorption = AbsorptionTensor::uniaxial(
        vec![
            band(585.0, 2_071.495_8, 2.01),
            band(744.0, 1_334.284_5, 0.887),
            band(457.0, 5_095.930_7, 0.433),
        ],
        vec![
            band(612.0, 3_170.518_3, 0.371),
            band(729.0, 2_104.194_8, 1.03),
            band(467.0, 4_743.037_6, 0.524),
        ],
    );
    material.with_absorption_path_scale(0.473_986_48)
}

/// The Daylight studio of that benchmark job: head shadow 16 degrees, surface glare 1.
#[must_use]
pub const fn bench_daylight() -> EnvironmentSource<'static> {
    LightingPreset::Daylight
        .studio(1.0, 0.85, 0.95)
        .with_head_shadow(16.0)
        .with_surface_glare(1.0)
}

/// The z scores (X, Y and Z of every position) of two sample streams over `positions` positions.
///
/// `first(position, i)` and `second(position, i)` give sample `i` of the two streams at a
/// position; the streams have `samples.0` and `samples.1` samples. A component where neither
/// stream varies has no z score and is skipped. This is the engine-free core of the path cache's
/// `equivalence_zs`, for tests that have no `Shader` (the beam tests).
pub fn compare_streams(
    positions: usize,
    samples: (u32, u32),
    mut first: impl FnMut(usize, u32) -> Vec3,
    mut second: impl FnMut(usize, u32) -> Vec3,
) -> Vec<f64> {
    let mut zs = Vec::new();
    for position in 0..positions {
        let mut a = Stats::default();
        for i in 0..samples.0 {
            a.add(first(position, i));
        }
        let mut b = Stats::default();
        for i in 0..samples.1 {
            b.add(second(position, i));
        }
        zs.extend((0..3).filter_map(|component| a.z_against(&b, component)));
    }
    zs
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::{
        NULL_OVER_3_SIGMA_RATE, SIGMA_THRESHOLD, Stats, assert_z_stats, compare_streams, draw,
        smooth_environment,
    };

    #[test]
    fn stats_mean_and_z_of_known_streams() {
        let mut a = Stats::default();
        let mut b = Stats::default();
        for v in [1.0_f32, 2.0, 3.0, 4.0] {
            a.add(Vec3::splat(v));
            b.add(Vec3::splat(v + 10.0));
        }
        assert!((a.mean(0) - 2.5).abs() < 1e-12);
        assert!((b.mean(2) - 12.5).abs() < 1e-12);
        // Both variances of the mean are (5/3) / 4; the difference is -10.
        let spread = (2.0 * (5.0 / 3.0) / 4.0_f64).sqrt();
        let z = a.z_against(&b, 1).expect("both streams vary");
        assert!((z + 10.0 / spread).abs() < 1e-9, "z = {z}");
    }

    #[test]
    fn z_is_none_for_constant_streams() {
        let mut a = Stats::default();
        let mut b = Stats::default();
        for _ in 0..4 {
            a.add(Vec3::ONE);
            b.add(Vec3::ONE);
        }
        assert_eq!(a.z_against(&b, 0), None);
    }

    #[test]
    fn assert_z_stats_accepts_small_and_rejects_large() {
        assert_z_stats(&[0.1, -0.5, 1.0, -1.2], 4.0, "small");
        let result = std::panic::catch_unwind(|| assert_z_stats(&[0.1, 5.0], 4.0, "large"));
        assert!(result.is_err());
        let result = std::panic::catch_unwind(|| assert_z_stats(&[], 4.0, "empty"));
        assert!(result.is_err());
    }

    #[test]
    fn draw_is_deterministic_and_stratified_in_unit_interval() {
        for i in 0..64 {
            let (seed, hero) = draw(i, 7);
            assert_eq!((seed, hero), draw(i, 7));
            assert!((0.0..=1.0).contains(&hero), "hero {hero}");
        }
        assert_ne!(draw(1, 7).0, draw(1, 8).0);
    }

    #[test]
    fn smooth_environment_builds() {
        let _environment = smooth_environment();
    }

    #[test]
    fn compare_streams_of_equal_noise_is_small() {
        // The same pseudo-random values in both streams: every difference of means is zero.
        let value = |position: usize, i: u32| {
            let (seed, hero) = draw(i, u32::try_from(position).expect("small position"));
            Vec3::new(hero, (seed & 0xFF) as f32 / 255.0, 0.5)
        };
        let zs = compare_streams(3, (200, 200), value, value);
        // The constant third component has no z score: two per position remain.
        assert_eq!(zs.len(), 6);
        assert!(zs.iter().all(|z| z.abs() < 1e-9), "{zs:?}");
        assert_z_stats(&zs, 4.0, "equal noise");
    }

    #[test]
    fn constants_have_the_documented_values() {
        assert_eq!(SIGMA_THRESHOLD.to_bits(), 3.0_f64.to_bits());
        assert_eq!(NULL_OVER_3_SIGMA_RATE.to_bits(), 0.0027_f64.to_bits());
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn constants_match_the_gpu_z_stats() {
        use crate::renderer::gpu::z_stats;
        assert_eq!(
            SIGMA_THRESHOLD.to_bits(),
            z_stats::SIGMA_THRESHOLD.to_bits()
        );
        assert_eq!(
            NULL_OVER_3_SIGMA_RATE.to_bits(),
            z_stats::NULL_OVER_3_SIGMA_RATE.to_bits()
        );
    }
}
