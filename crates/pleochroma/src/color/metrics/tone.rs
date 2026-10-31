//! The face-up tone: the CIELAB colour of the light a stone returns to the observer at one
//! pose.
//!
//! Each visibly returned ray carries a Fresnel weight `w_i` and an internal path `L_i`
//! (model units). The mean transmittance spectrum of the returned light is the mixture
//! `T(lambda) = sum_i w_i exp(-alpha(lambda) s L_i) / sum_i w_i`, not Beer-Lambert of the
//! mean path (the mixture is lighter and less saturated, which is physically right: short
//! paths dominate the brightness). `s` is [`GemMaterial::absorption_path_scale`], the same
//! size rule the renderer uses.
//!
//! The mixture is linear in the rays, so [`ToneAccumulator`] only keeps a fixed
//! [`PATH_BINS`]-bin histogram of the weights over `0 ..= MAX_PATH_HALF_WIDTHS` girdle
//! half-widths (a longer path lands in the last bin); the spectrum is then summed over the
//! bin centres, independent of the ray count. The colorimetry runs under the lighting
//! preset's own light ([`ToneIlluminant`]).
//!
//! A pleochroic material (see `color::principal_axes`) cannot use one path per ray: the
//! absorption depends on how the path splits over the crystal's principal axes. Its
//! accumulator ([`ToneAccumulator::with_principal_paths`]) keeps the per-ray list
//! ([`PrincipalRay`]: weight, path, per-axis path) instead of the histogram, and the mixture is
//! `T(lambda) = sum_i w_i exp(-s sum_p alpha_p(lambda) D_ip) / sum_i w_i`
//! ([`principal_transmittance`]).

use glam::Vec3;

use crate::{
    color::{body_color::body_color_from_spectra, principal_axes::principal_alphas},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, LightingPreset,
            environment::{d65_relative_spectral_power, environment_white_balance},
        },
    },
};

/// Number of path-length bins of the tone histogram.
pub(super) const PATH_BINS: usize = 128;

/// The histogram spans `0 ..= MAX_PATH_HALF_WIDTHS` girdle half-widths of internal path;
/// a ray that travels further lands in the last bin.
pub(super) const MAX_PATH_HALF_WIDTHS: f32 = 12.0;

/// The face-up colour of the light a stone returns to the eye at one pose.
///
/// Lab values are relative to the illuminant's own white, so a colourless stone is
/// neutral (`l_star` 100, `chroma` 0) under every lighting preset; the `srgb` swatch is
/// adapted to the screen's D65 the way the Live Render white-balances that preset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceUpTone {
    /// CIELAB lightness: 100 = colourless, 0 = black (no returned light at all).
    pub l_star: f32,
    /// CIELAB chroma `C*ab = sqrt(a*^2 + b*^2)`: the colour strength.
    pub chroma: f32,
    /// CIELAB hue angle in degrees, `0 ..= 360`; for display only (0 when `chroma < 0.5`).
    pub hue_deg: f32,
    /// The swatch: display sRGB of the returned light.
    pub srgb: [u8; 3],
    /// `sum w_i L_i / sum w_i`, in model units: how far the returned light travels
    /// through the stone on average (independent of the material and the size scale).
    pub mean_path_units: f32,
    /// `returned / total` rays, the same figure as `brilliance_pct / 100`.
    pub returned_fraction: f32,
}

impl FaceUpTone {
    /// No returned ray at all: black, every field zero.
    pub const NONE: Self = Self {
        l_star: 0.0,
        chroma: 0.0,
        hue_deg: 0.0,
        srgb: [0, 0, 0],
        mean_path_units: 0.0,
        returned_fraction: 0.0,
    };
}

/// The light the tone's colorimetry is taken under.
///
/// A lighting preset's own spectrum and white point, with a documented fall-back to D65
/// where the preset has no visible white point (the UV lamps) or no preset at all (an HDR
/// map).
///
/// Everything here is a pure function of the preset, so the same preset always gives the
/// same bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToneIlluminant {
    /// The preset whose light the tone is measured under.
    pub preset: LightingPreset,
    /// The tone is measured under D65 instead of the preset's own light (UV lamps, HDR
    /// maps: no visible white point).
    pub fallback_to_d65: bool,
}

impl ToneIlluminant {
    /// The illuminant of an environment: a studio preset is used as it is (UV lamps fall
    /// back to D65); an HDR map has no single white point and falls back to D65.
    #[must_use]
    pub const fn for_environment(environment: EnvironmentSource<'_>) -> Self {
        match environment {
            EnvironmentSource::Studio { preset, .. } => Self {
                preset,
                fallback_to_d65: preset.is_uv_lamp(),
            },
            EnvironmentSource::HdrMap(_) => Self {
                preset: LightingPreset::Daylight,
                fallback_to_d65: true,
            },
        }
    }

    /// Whether the tone is under the preset's own light (`false` when it fell back to
    /// D65); for the UI note.
    #[must_use]
    pub const fn uses_preset_light(&self) -> bool {
        !self.fallback_to_d65
    }

    /// Relative spectral power at `lambda_nm`: the D65 table when falling back or when the
    /// preset itself is a D65 one, else the preset's own Planckian fit.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spectral tables take an f32 wavelength; nanometres are in the normal float range"
    )]
    pub fn spectral_power(&self, lambda_nm: f64) -> f64 {
        if self.fallback_to_d65 || self.preset.uses_d65() {
            f64::from(d65_relative_spectral_power(lambda_nm as f32))
        } else {
            f64::from(self.preset.spectral_power(lambda_nm as f32))
        }
    }

    /// The per-cone von Kries scale that adapts the preset's white to the screen's D65, the
    /// renderer's own cached Bradford scale; `None` where the renderer applies none (the
    /// D65 presets) or the tone fell back to D65.
    #[must_use]
    pub fn display_adaptation(&self) -> Option<Vec3> {
        if !self.fallback_to_d65 && self.preset.uses_white_balance() {
            Some(environment_white_balance(self.preset.studio(1.0, 0.0, 0.0)))
        } else {
            None
        }
    }
}

/// One visibly returned ray as a pleochroic tone keeps it: its Fresnel weight, its total
/// internal path and that path split over the absorption tensor's three principal axes
/// (`color::principal_axes` slot order; the split sums to `path_len`).
///
/// This is the per-ray record the doubling tracer builds on: an ordinary-only ray has
/// `principal_path[2] == 0` by construction, an extraordinary one carries `(e_e . c)^2` of its
/// path along `c`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PrincipalRay {
    /// Fresnel entry-times-exit transmittance of the ray.
    pub(super) weight: f32,
    /// Total internal path, model units.
    pub(super) path_len: f32,
    /// Internal path per principal axis, model units.
    pub(super) principal_path: [f32; 3],
}

/// Collects the returned rays of one evaluation, in grid order: into the path histogram, or,
/// for a pleochroic material ([`Self::with_principal_paths`]), into the per-ray list.
pub(super) struct ToneAccumulator {
    /// Fresnel weight summed per path-length bin (unused while `rays` is `Some`).
    bins: [f32; PATH_BINS],
    /// The returned rays with their per-axis paths; `Some` only for a pleochroic material.
    rays: Option<Vec<PrincipalRay>>,
    /// `sum w_i L_i`.
    weighted_path: f32,
    /// `sum w_i`.
    weight_sum: f32,
    /// Visibly returned rays offered.
    returned: u32,
    /// Every traced ray (the denominator of the returned fraction).
    total: u32,
    /// Path length per bin, model units.
    bin_width: f32,
}

impl ToneAccumulator {
    /// An empty accumulator for a stone with the given girdle half-width (model units).
    pub(super) fn new(half_width: f32) -> Self {
        Self {
            bins: [0.0; PATH_BINS],
            rays: None,
            weighted_path: 0.0,
            weight_sum: 0.0,
            returned: 0,
            total: 0,
            bin_width: MAX_PATH_HALF_WIDTHS * half_width / PATH_BINS as f32,
        }
    }

    /// An empty accumulator for a pleochroic material: it keeps every returned ray with its
    /// per-axis path (at most 18 x 18 x 5 entries) instead of the histogram, and
    /// [`Self::finish`] evaluates `T(lambda) = sum_i w_i exp(-s sum_p alpha_p(lambda) D_ip) /
    /// sum_i w_i`. Feed it with [`Self::offer_principal`].
    pub(super) fn with_principal_paths(half_width: f32) -> Self {
        Self {
            rays: Some(Vec::new()),
            ..Self::new(half_width)
        }
    }

    /// Whether this accumulator wants the per-axis path of every returned ray (it was built by
    /// [`Self::with_principal_paths`]); the tracer only computes the split when it does.
    pub(super) const fn wants_principal_paths(&self) -> bool {
        self.rays.is_some()
    }

    /// The returned rays kept so far with their per-axis paths, in grid order; empty for a
    /// histogram accumulator.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by the doubling tracer (lane CA-C3), which splits these rays by mode"
        )
    )]
    pub(super) fn principal_rays(&self) -> &[PrincipalRay] {
        self.rays.as_deref().unwrap_or_default()
    }

    /// Adds one returned ray: [`Self::offer`] for a histogram accumulator (`principal_path`
    /// ignored, bit for bit the old behaviour), else the ray joins the per-axis list.
    pub(super) fn offer_principal(
        &mut self,
        transmittance: f32,
        path_len: f32,
        principal_path: [f32; 3],
    ) {
        let Some(rays) = self.rays.as_mut() else {
            self.offer(transmittance, path_len);
            return;
        };
        rays.push(PrincipalRay {
            weight: transmittance,
            path_len,
            principal_path,
        });
        self.weighted_path = path_len.mul_add(transmittance, self.weighted_path);
        self.weight_sum += transmittance;
        self.returned += 1;
    }

    /// Counts one traced ray (every classified ray, returned or not).
    pub(super) const fn count_total(&mut self) {
        self.total += 1;
    }

    /// Adds one returned ray with Fresnel weight `transmittance` and internal path
    /// `path_len` (model units).
    pub(super) fn offer(&mut self, transmittance: f32, path_len: f32) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a non-negative path over the bin width; `as` saturates (NaN gives 0) and the bin is clamped below"
        )]
        let bin = ((path_len / self.bin_width) as usize).min(PATH_BINS - 1);
        self.bins[bin] += transmittance;
        self.weighted_path = path_len.mul_add(transmittance, self.weighted_path);
        self.weight_sum += transmittance;
        self.returned += 1;
    }

    /// Turns the histogram (or, for a pleochroic material, the per-ray list) into the tone of
    /// `material` under `illuminant`.
    pub(super) fn finish(&self, material: &GemMaterial, illuminant: ToneIlluminant) -> FaceUpTone {
        let returned_fraction = self.returned as f32 / self.total.max(1) as f32;
        if self.weight_sum <= 0.0 {
            return FaceUpTone {
                returned_fraction,
                ..FaceUpTone::NONE
            };
        }

        // Occupied bins only: (normalised weight, bin-centre path scaled to absorption units).
        let scale = f64::from(material.absorption_path_scale.max(0.0));
        let weight_sum = f64::from(self.weight_sum);
        let occupied: Vec<(f64, f64)> = self
            .bins
            .iter()
            .enumerate()
            .filter(|&(_, &w)| w > 0.0)
            .map(|(b, &w)| {
                let centre = (b as f32 + 0.5) * self.bin_width;
                (f64::from(w) / weight_sum, f64::from(centre) * scale)
            })
            .collect();

        #[expect(
            clippy::cast_possible_truncation,
            reason = "band evaluation takes an f32 wavelength; nanometres are in the normal float range"
        )]
        let alpha_of = |bands: &[AbsorptionBand], lambda: f64| -> f64 {
            f64::from(
                bands
                    .iter()
                    .map(|band| band.evaluate(lambda as f32))
                    .sum::<f32>(),
            )
            .max(0.0)
        };
        let mixture = |alpha: f64| -> f64 {
            occupied
                .iter()
                .fold(0.0, |acc, &(w, path)| w.mul_add((-alpha * path).exp(), acc))
        };

        let tensor = &material.absorption;
        // The unpolarised rule of `body_color::body_colors`.
        let t_fn = |lambda: f64| -> f64 {
            let t_o = mixture(alpha_of(&tensor.o_ray, lambda));
            tensor.beta_ray.as_ref().map_or_else(
                || {
                    if tensor.is_pleochroic {
                        let t_e = mixture(alpha_of(&tensor.e_ray, lambda));
                        2.0f64.mul_add(t_o, t_e) / 3.0
                    } else {
                        t_o
                    }
                },
                |beta_bands| {
                    let t_b = mixture(alpha_of(beta_bands, lambda));
                    let t_e = mixture(alpha_of(&tensor.e_ray, lambda));
                    (t_o + t_b + t_e) / 3.0
                },
            )
        };

        // A pleochroic material's list of rays replaces the (then empty) histogram.
        let colour = self.rays.as_ref().map_or_else(
            || {
                body_color_from_spectra(
                    t_fn,
                    |lambda| illuminant.spectral_power(lambda),
                    illuminant.display_adaptation(),
                )
            },
            |rays| {
                body_color_from_spectra(
                    |lambda| principal_transmittance(rays, tensor, scale, lambda),
                    |lambda| illuminant.spectral_power(lambda),
                    illuminant.display_adaptation(),
                )
            },
        );

        let [l, a, b] = colour.lab;
        let chroma = a.hypot(b);
        let hue_deg = if chroma < 0.5 {
            0.0
        } else {
            let deg = b.atan2(a).to_degrees();
            if deg < 0.0 { deg + 360.0 } else { deg }
        };

        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the tone's storage precision; L*, chroma and hue are in the normal float range"
        )]
        let (l_star, chroma_f32, hue_f32) = (l as f32, chroma as f32, hue_deg as f32);
        FaceUpTone {
            l_star,
            chroma: chroma_f32,
            hue_deg: hue_f32,
            srgb: colour.srgb,
            mean_path_units: self.weighted_path / self.weight_sum,
            returned_fraction,
        }
    }
}

/// The mean transmittance spectrum of a pleochroic material's returned rays at `lambda_nm`:
/// `sum_i w_i exp(-s sum_p alpha_p(lambda) D_ip) / sum_i w_i`, with `s` the absorption path
/// scale (`scale`), `alpha_p` the tensor's principal coefficients
/// ([`principal_alphas`]) and `D_ip` each ray's path along principal axis `p`. Zero when the
/// rays carry no weight. The one place the per-ray principal paths turn into a spectrum, so the
/// doubling tracer can reuse it per mode.
pub(super) fn principal_transmittance(
    rays: &[PrincipalRay],
    tensor: &AbsorptionTensor,
    scale: f64,
    lambda_nm: f64,
) -> f64 {
    let alpha = principal_alphas(tensor, lambda_nm);
    let (sum, weight_sum) = rays.iter().fold((0.0f64, 0.0f64), |(sum, weights), ray| {
        let w = f64::from(ray.weight);
        let depth = scale
            * alpha[2].mul_add(
                f64::from(ray.principal_path[2]),
                alpha[1].mul_add(
                    f64::from(ray.principal_path[1]),
                    alpha[0] * f64::from(ray.principal_path[0]),
                ),
            );
        (w.mul_add((-depth).exp(), sum), weights + w)
    });
    if weight_sum > 0.0 {
        sum / weight_sum
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::body_color::{Illuminant, body_color},
        optics::materials::body_color::BODY_COLOR_PRESETS,
    };

    fn bits3(v: Vec3) -> [u32; 3] {
        v.to_array().map(f32::to_bits)
    }

    #[test]
    fn the_mixture_is_lighter_than_the_mean_path() {
        let material = GemMaterial::diamond().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb);
        let mut acc = ToneAccumulator::new(1.0);
        for path in [0.5, 4.0] {
            acc.count_total();
            acc.offer(1.0, path);
        }
        let illuminant =
            ToneIlluminant::for_environment(LightingPreset::Daylight.studio(1.0, 0.85, 0.95));
        let tone = acc.finish(&material, illuminant);

        #[expect(
            clippy::cast_possible_truncation,
            reason = "band evaluation takes an f32 wavelength; nanometres are in the normal float range"
        )]
        let alpha = |lambda: f64| -> f64 {
            f64::from(
                material
                    .absorption
                    .o_ray
                    .iter()
                    .map(|band| band.evaluate(lambda as f32))
                    .sum::<f32>(),
            )
        };
        let mean_path = body_color(alpha, 2.25, Illuminant::D65);
        assert!(
            f64::from(tone.l_star) > mean_path.lab[0],
            "mixture L* {} must exceed Beer-Lambert of the mean path {}",
            tone.l_star,
            mean_path.lab[0]
        );
        assert!((tone.mean_path_units - 2.25).abs() < 1e-5);
    }

    fn tone_bits(t: FaceUpTone) -> [u32; 7] {
        [
            t.l_star.to_bits(),
            t.chroma.to_bits(),
            t.hue_deg.to_bits(),
            u32::from(t.srgb[0]),
            u32::from(t.srgb[1]),
            t.mean_path_units.to_bits(),
            t.returned_fraction.to_bits(),
        ]
    }

    /// A non-pleochroic accumulator (the histogram path) takes the principal path as a no-op:
    /// the tone is bit-identical to feeding the same rays through `offer`.
    #[test]
    fn a_histogram_accumulator_ignores_the_principal_path() {
        let material = GemMaterial::diamond().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb);
        let illuminant =
            ToneIlluminant::for_environment(LightingPreset::Daylight.studio(1.0, 0.85, 0.95));
        let mut plain = ToneAccumulator::new(1.0);
        let mut offered = ToneAccumulator::new(1.0);
        for (weight, path) in [(0.9f32, 0.5f32), (0.7, 2.5), (0.8, 4.0), (0.6, 9.0)] {
            plain.count_total();
            offered.count_total();
            plain.offer(weight, path);
            offered.offer_principal(weight, path, [path, 0.0, 0.0]);
        }
        assert!(!offered.wants_principal_paths());
        assert_eq!(offered.principal_rays(), []);
        assert_eq!(
            tone_bits(plain.finish(&material, illuminant)),
            tone_bits(offered.finish(&material, illuminant))
        );
    }

    /// For an isotropic tensor the per-axis split is irrelevant: any split of a path gives the
    /// plain Beer-Lambert mixture over the total paths.
    #[test]
    fn an_isotropic_tensor_ignores_how_the_path_splits() {
        let tensor = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(550.0, 40.0, 0.4)]);
        let rays = [
            PrincipalRay {
                weight: 0.5,
                path_len: 2.0,
                principal_path: [1.0, 0.5, 0.5],
            },
            PrincipalRay {
                weight: 1.0,
                path_len: 1.0,
                principal_path: [0.3, 0.3, 0.4],
            },
        ];
        let scale = 1.3;
        let alpha = principal_alphas(&tensor, 550.0)[0];
        let expected =
            0.5f64.mul_add((-alpha * 2.0 * scale).exp(), (-alpha * 1.0 * scale).exp()) / 1.5;
        let got = principal_transmittance(&rays, &tensor, scale, 550.0);
        assert!((got - expected).abs() < 1e-6, "{got} vs {expected}");
    }

    /// A pleochroic accumulator keeps the rays, and moving the same path from the ordinary to
    /// the extraordinary axis changes the tone.
    #[test]
    fn a_pleochroic_accumulator_depends_on_the_axis_split() {
        let mut material = GemMaterial::sapphire();
        material.absorption = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(580.0, 60.0, 0.5)],
            vec![AbsorptionBand::new(450.0, 50.0, 0.5)],
        );
        let illuminant =
            ToneIlluminant::for_environment(LightingPreset::Daylight.studio(1.0, 0.85, 0.95));
        let tone_of = |split: [f32; 3]| {
            let mut acc = ToneAccumulator::with_principal_paths(1.0);
            assert!(acc.wants_principal_paths());
            for path in [1.5f32, 2.5] {
                acc.count_total();
                acc.offer_principal(0.8, path, split.map(|s| s * path));
            }
            assert_eq!(acc.principal_rays().len(), 2);
            assert_eq!(acc.principal_rays()[1].path_len, 2.5);
            acc.finish(&material, illuminant)
        };
        let ordinary = tone_of([1.0, 0.0, 0.0]);
        let extraordinary = tone_of([0.0, 0.0, 1.0]);
        assert_ne!(ordinary.srgb, extraordinary.srgb);
        assert_eq!(
            ordinary.mean_path_units.to_bits(),
            extraordinary.mean_path_units.to_bits()
        );
    }

    #[test]
    fn uv_lamps_and_hdr_maps_fall_back_to_d65() {
        let uv = ToneIlluminant::for_environment(LightingPreset::UvLamp365.studio(1.0, 0.85, 0.95));
        assert!(uv.fallback_to_d65);
        assert!(!uv.uses_preset_light());
        assert_eq!(
            uv.spectral_power(550.0).to_bits(),
            f64::from(d65_relative_spectral_power(550.0)).to_bits()
        );
        assert!(uv.display_adaptation().is_none());

        for preset in [LightingPreset::Daylight, LightingPreset::Incandescent] {
            let ill = ToneIlluminant::for_environment(preset.studio(1.0, 0.85, 0.95));
            assert!(!ill.fallback_to_d65);
            assert!(ill.uses_preset_light());
            assert_eq!(
                ill.spectral_power(550.0).to_bits(),
                f64::from(preset.spectral_power(550.0)).to_bits()
            );
        }

        for preset in [
            LightingPreset::Daylight,
            LightingPreset::IsoHemisphere,
            LightingPreset::DaylightDome,
            LightingPreset::UvLamp365,
            LightingPreset::UvLamp395,
        ] {
            let ill = ToneIlluminant::for_environment(preset.studio(1.0, 0.85, 0.95));
            assert!(ill.display_adaptation().is_none(), "{preset:?}");
        }
        for preset in [
            LightingPreset::Incandescent,
            LightingPreset::RingLights,
            LightingPreset::DarkSpotlight,
            LightingPreset::LightTent,
        ] {
            let ill = ToneIlluminant::for_environment(preset.studio(1.0, 0.85, 0.95));
            let expected = environment_white_balance(preset.studio(1.0, 0.85, 0.95));
            let got = ill.display_adaptation().expect("Planckian preset adapts");
            assert_eq!(bits3(got), bits3(expected), "{preset:?}");
        }
    }
}
