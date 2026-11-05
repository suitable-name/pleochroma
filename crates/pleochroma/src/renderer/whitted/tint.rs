//! Spectral absorption for the deterministic tracer, resolved once per scene.
//!
//! The Ray-Traced engine carries RGB, but a stone's color depends on how far light travels
//! through it, and that dependence is spectral: a long path narrows the transmitted light
//! towards the material's most transparent wavelengths. Three RGB coefficients fitted at one
//! path length reproduce that length only, and as `exp(-alpha * length)` they can never go
//! below zero, so a color outside the sRGB gamut (an emerald's green needs a negative red)
//! was cut back to the gamut edge. Together an emerald came out 10-25 degrees of hue too
//! yellow and at half its chroma (2026-11-02, against the spectral renderer).
//!
//! [`BinnedAbsorption`] keeps the spectrum instead: forty 10 nm wavelength bins, each with its
//! own optical depth per model unit and its share of the illuminant's linear-sRGB white (which
//! may be negative in a channel). A branch carries only how far it travelled inside (`length`)
//! and how much of that ran along the optic axis (`axial`); [`BinnedAbsorption::transmission`]
//! turns those two numbers into the RGB transmission of the whole path. Against the 5 nm
//! spectral integral an emerald stays within 0.5 degrees of hue, 1 % of chroma and 0.6 L*
//! from half to six face-up paths (25 nm bins lost 6-8 % of the chroma). Unpolarized light is
//! split evenly over the absorption modes (one isotropic, the ordinary and extraordinary
//! waves of a uniaxial stone, or the three principal waves of a biaxial one), each attenuated
//! on its own.

use glam::Vec3;

use crate::{
    color::{cie1931::cie_1931_cmf, space::ColorSpace},
    optics::{absorption::AbsorptionBand, raytracer::EnvironmentSource},
    render_setup::MODEL_UNIT_FACE_UP_PATH,
    simd::exp_f32x8,
};

/// Wavelength bins of [`BinnedAbsorption`]: 10 nm each over 380-780 nm. A multiple of eight
/// (the vector exponential's width) and of four (the GPU twin's vec4 slots).
pub const ABSORPTION_BINS: usize = 40;
/// 5 nm integration steps per bin.
const STEPS_PER_BIN: u16 = 2;
/// The most absorption modes a material has (biaxial: three principal waves).
pub const ABSORPTION_MODES: usize = 3;

const _: () = assert!(ABSORPTION_BINS.is_multiple_of(8));

/// The optical depth per model unit of a bin that lets nothing through even on the
/// reference path: every `exp` of it underflows to (nearly) zero.
const OPAQUE_DEPTH: f32 = 1.0e3;

/// The bin a 5 nm integration step (`0..=80`, 380-780 nm) falls in; 780 nm joins the last.
fn bin_of(step: u16) -> usize {
    let bin = usize::from(step / STEPS_PER_BIN);
    if bin < ABSORPTION_BINS {
        bin
    } else {
        ABSORPTION_BINS - 1
    }
}

/// Integrates the same visible CMFs and normalization as the spectral renderer.
pub(super) fn integrate_rgb(mut spectrum: impl FnMut(f32) -> f32) -> Vec3 {
    let mut xyz = Vec3::ZERO;
    for step in 0_u16..=80 {
        let wavelength = f32::from(step).mul_add(5.0, 380.0);
        xyz += Vec3::from_array(cie_1931_cmf(wavelength)) * spectrum(wavelength) * 5.0;
    }
    ColorSpace::Srgb.xyz_to_linear(xyz / 106.856)
}

pub(super) fn illuminant(environment: EnvironmentSource<'_>, wavelength: f32) -> f32 {
    match environment {
        EnvironmentSource::Studio { preset, .. } => preset.spectral_power(wavelength),
        EnvironmentSource::HdrMap(_) => {
            crate::optics::raytracer::environment::d65_relative_spectral_power(wavelength)
        }
    }
}

pub(super) fn illuminant_rgb(environment: EnvironmentSource<'_>) -> Vec3 {
    integrate_rgb(|wavelength| illuminant(environment, wavelength))
}

/// A material's absorption as the Ray-Traced engine applies it (see the module doc comment).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinnedAbsorption {
    /// Absorption modes in use: `0` (colorless, transmission one), `1`, `2` or `3`.
    pub modes: u8,
    /// Per mode and bin, the optical depth per model unit of path (`p`) and per model unit of
    /// axis-weighted path (`q`): a branch's optical depth is `p * length + q * axial`.
    pub p: [[f32; ABSORPTION_BINS]; ABSORPTION_MODES],
    /// See [`Self::p`]; zero except for the extraordinary mode of a uniaxial stone.
    pub q: [[f32; ABSORPTION_BINS]; ABSORPTION_MODES],
    /// Each bin's share of the illuminant's linear-sRGB white, per channel: the bins sum to
    /// one in every channel, so a path without absorption transmits exactly one.
    pub weights: [Vec3; ABSORPTION_BINS],
    /// Unit optic axis of a uniaxial stone (`axial` measures the path along it).
    pub axis: Vec3,
}

impl BinnedAbsorption {
    /// No absorption at all.
    pub const NONE: Self = Self {
        modes: 0,
        p: [[0.0; ABSORPTION_BINS]; ABSORPTION_MODES],
        q: [[0.0; ABSORPTION_BINS]; ABSORPTION_MODES],
        weights: [Vec3::ZERO; ABSORPTION_BINS],
        axis: Vec3::Y,
    };

    /// Builds the bins for an isotropic (`[o]`), uniaxial (`[o, e]` with `uniaxial_axis`) or
    /// biaxial (`[o, beta, e]`) material, whose band coefficients times `path_scale` are the
    /// optical depth per model unit (the material's `absorption_path_scale`), under
    /// `environment`'s illuminant.
    #[must_use]
    pub fn new(
        modes: &[&[AbsorptionBand]],
        uniaxial_axis: Option<Vec3>,
        path_scale: f32,
        environment: EnvironmentSource<'_>,
    ) -> Self {
        if path_scale <= 0.0 || modes.iter().all(|bands| bands.is_empty()) {
            return Self::NONE;
        }
        let mut out = Self::NONE;
        out.weights = bin_weights(environment);
        for (mode, bands) in modes.iter().take(ABSORPTION_MODES).enumerate() {
            out.p[mode] = bin_depths(bands, path_scale, environment);
        }
        match uniaxial_axis {
            Some(axis) => {
                // Ordinary wave: `o` whatever the direction. Extraordinary wave: `o` along
                // the axis, `e` across it, `o cos^2 + e sin^2` between, which is
                // `e * length + (o - e) * axial`.
                let (o, e) = (out.p[0], out.p[1]);
                out.q[1] = std::array::from_fn(|bin| o[bin] - e[bin]);
                out.axis = axis;
                out.modes = 2;
            }
            None => {
                out.modes = u8::try_from(modes.len().min(ABSORPTION_MODES)).unwrap_or(1);
            }
        }
        out
    }

    /// The axis-weighted length a segment of `length` along unit `dir` adds: `cos^2` of its
    /// angle to the optic axis times its length. Plain glam `dot`, like the WGSL twin.
    #[must_use]
    pub fn axial(&self, dir: Vec3, length: f32) -> f32 {
        let cosine = dir.dot(self.axis);
        (cosine * cosine) * length
    }

    /// The RGB transmission of a path `length` model units long, `axial` of it along the
    /// optic axis, averaged over the modes. Deliberately unclamped: a saturated stone's
    /// transmission is negative in a channel exactly when its color lies outside the sRGB
    /// gamut, and the tracer's XYZ output keeps that color for the display rule to map. Every
    /// channel stays between the sums of its negative and its positive bin weights.
    #[must_use]
    pub fn transmission(&self, length: f32, axial: f32) -> Vec3 {
        if self.modes == 0 {
            return Vec3::ONE;
        }
        let mut sum = Vec3::ZERO;
        for mode in 0..usize::from(self.modes) {
            for first in (0..ABSORPTION_BINS).step_by(8) {
                let exponents: [f32; 8] = std::array::from_fn(|lane| {
                    let bin = first + lane;
                    -self.p[mode][bin].mul_add(length, self.q[mode][bin] * axial)
                });
                for (lane, transmitted) in exp_f32x8(exponents).into_iter().enumerate() {
                    sum = self.weights[first + lane].mul_add(Vec3::splat(transmitted), sum);
                }
            }
        }
        sum * (1.0 / f32::from(self.modes))
    }

    /// A model with one bin per RGB channel, so `transmission(length, _)` is exactly
    /// `exp(-alpha * length)` per channel: what the chain tests compare against.
    #[cfg(test)]
    pub(super) fn rgb_for_tests(alpha: Vec3) -> Self {
        if alpha == Vec3::ZERO {
            return Self::NONE;
        }
        let mut out = Self::NONE;
        out.modes = 1;
        out.p[0][0] = alpha.x;
        out.p[0][1] = alpha.y;
        out.p[0][2] = alpha.z;
        out.weights[0] = Vec3::X;
        out.weights[1] = Vec3::Y;
        out.weights[2] = Vec3::Z;
        out
    }
}

/// Each bin's share of `environment`'s illuminant white, per linear-sRGB channel.
fn bin_weights(environment: EnvironmentSource<'_>) -> [Vec3; ABSORPTION_BINS] {
    let mut xyz = [Vec3::ZERO; ABSORPTION_BINS];
    for step in 0_u16..=80 {
        let wavelength = f32::from(step).mul_add(5.0, 380.0);
        xyz[bin_of(step)] +=
            Vec3::from_array(cie_1931_cmf(wavelength)) * illuminant(environment, wavelength) * 5.0;
    }
    let rgb = xyz.map(|bin| ColorSpace::Srgb.xyz_to_linear(bin / 106.856));
    let white = rgb.iter().copied().sum::<Vec3>();
    let share = |value: f32, total: f32| if total > 1.0e-6 { value / total } else { 0.0 };
    rgb.map(|bin| {
        Vec3::new(
            share(bin.x, white.x),
            share(bin.y, white.y),
            share(bin.z, white.z),
        )
    })
}

/// The optical depth per model unit of each bin for one mode's `bands`: the depth that
/// transmits, through the face-up reference path, what the bin's 5 nm steps transmit
/// together, weighted by how much each step is seen (illuminant times the summed CMFs).
/// Matching the bin at that path rather than averaging its coefficient keeps a narrow band
/// from darkening the whole bin.
fn bin_depths(
    bands: &[AbsorptionBand],
    path_scale: f32,
    environment: EnvironmentSource<'_>,
) -> [f32; ABSORPTION_BINS] {
    let mut seen = [0.0_f64; ABSORPTION_BINS];
    let mut transmitted = [0.0_f64; ABSORPTION_BINS];
    for step in 0_u16..=80 {
        let wavelength = f32::from(step).mul_add(5.0, 380.0);
        let cmf = cie_1931_cmf(wavelength);
        let weight = f64::from(illuminant(environment, wavelength) * (cmf[0] + cmf[1] + cmf[2]));
        let coefficient: f32 = bands.iter().map(|band| band.evaluate(wavelength)).sum();
        let depth = f64::from(coefficient.max(0.0) * path_scale * MODEL_UNIT_FACE_UP_PATH);
        seen[bin_of(step)] += weight;
        transmitted[bin_of(step)] += weight * (-depth).exp();
    }
    std::array::from_fn(|bin| {
        if seen[bin] <= 0.0 {
            return 0.0;
        }
        if transmitted[bin] <= 0.0 {
            // Opaque at the reference path already: opaque at every path.
            return OPAQUE_DEPTH;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an f64 sum rounded to the f32 depth the tracer uses, once per scene"
        )]
        let depth =
            (-(transmitted[bin] / seen[bin]).ln() / f64::from(MODEL_UNIT_FACE_UP_PATH)) as f32;
        depth.max(0.0)
    })
}
