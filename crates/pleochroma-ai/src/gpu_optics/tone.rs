//! MIT source: pleochroma/color/metrics/tone.rs. Production finishes compact GPU summaries
//! and spectra with the reference CIE transform; ordered CPU accumulation is test-only.
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
//! accumulator (`ToneAccumulator::with_principal_paths`) keeps the per-ray list
//! (`PrincipalRay`: weight, path, per-axis path) instead of the histogram, and the mixture is
//! `T(lambda) = sum_i w_i exp(-s sum_p alpha_p(lambda) D_ip) / sum_i w_i`
//! (the CPU reference is `principal_transmittance`).

use pleochroma::color::body_color::body_color_from_spectra;
#[cfg(test)]
use pleochroma::optics::absorption::{AbsorptionBand, AbsorptionTensor};
#[cfg(test)]
use pleochroma::{color::principal_axes::principal_alphas, optics::materials::GemMaterial};

use super::tone_cuda::ToneSpectrum;
#[cfg(test)]
use super::tone_cuda::{ToneInput, WAVELENGTHS};

/// Number of path-length bins of the tone histogram.
pub(super) const PATH_BINS: usize = 128;

/// The histogram spans `0 ..= MAX_PATH_HALF_WIDTHS` girdle half-widths of internal path;
/// a ray that travels further lands in the last bin.
pub(super) const MAX_PATH_HALF_WIDTHS: f32 = 12.0;

pub use pleochroma::color::metrics::{FaceUpTone, ToneIlluminant};

/// One visibly returned ray as a pleochroic tone keeps it: its Fresnel weight, its total
/// internal path and that path split over the absorption tensor's three principal axes
/// (`color::principal_axes` slot order; the split sums to `path_len`).
///
/// This is the per-ray record the doubling tracer builds on: an ordinary-only ray has
/// `principal_path[2] == 0` by construction, an extraordinary one carries `(e_e . c)^2` of its
/// path along `c`.
#[cfg(test)]
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
/// for a pleochroic material (`Self::with_principal_paths`), into the per-ray list.
pub(super) struct ToneAccumulator {
    /// Fresnel weight summed per path-length bin (unused while `rays` is `Some`).
    #[cfg(test)]
    bins: [f32; PATH_BINS],
    /// The returned rays with their per-axis paths; `Some` only for a pleochroic material.
    #[cfg(test)]
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
    #[cfg(test)]
    bin_width: f32,
}

impl ToneAccumulator {
    pub(super) fn from_gpu_summary(summary: &[f64; 7]) -> Self {
        Self {
            weighted_path: summary[0] as f32,
            weight_sum: summary[1] as f32,
            returned: summary[2] as u32,
            total: summary[3] as u32,
            #[cfg(test)]
            bins: [0.0; PATH_BINS],
            #[cfg(test)]
            rays: None,
            #[cfg(test)]
            bin_width: MAX_PATH_HALF_WIDTHS / PATH_BINS as f32,
        }
    }
    /// An empty accumulator for a stone with the given girdle half-width (model units).
    #[cfg(test)]
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
    /// The spectrum evaluates `T(lambda) = sum_i w_i exp(-s sum_p alpha_p(lambda) D_ip) /
    /// sum_i w_i`. Feed it with [`Self::offer_principal`].
    #[cfg(test)]
    pub(super) fn with_principal_paths(half_width: f32) -> Self {
        Self {
            rays: Some(Vec::new()),
            ..Self::new(half_width)
        }
    }

    /// The returned rays kept so far with their per-axis paths, in grid order; empty for a
    /// histogram accumulator.
    #[cfg(test)]
    pub(super) fn principal_rays(&self) -> &[PrincipalRay] {
        self.rays.as_deref().unwrap_or_default()
    }

    /// Adds one returned ray: [`Self::offer`] for a histogram accumulator (`principal_path`
    /// ignored, bit for bit the old behaviour), else the ray joins the per-axis list.
    #[cfg(test)]
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
    #[cfg(test)]
    pub(super) const fn count_total(&mut self) {
        self.total += 1;
    }

    /// Adds one returned ray with Fresnel weight `transmittance` and internal path
    /// `path_len` (model units).
    #[cfg(test)]
    pub(super) fn offer(&mut self, transmittance: f32, path_len: f32) {
        let bin = ((path_len / self.bin_width) as usize).min(PATH_BINS - 1);
        self.bins[bin] += transmittance;
        self.weighted_path = path_len.mul_add(transmittance, self.weighted_path);
        self.weight_sum += transmittance;
        self.returned += 1;
    }

    /// Preserve the reference ray/bin order and host absorption-band evaluation. Principal
    /// rays normalize by their f64 weight sum; occupied bins retain the reference's f32 sum.
    #[cfg(test)]
    pub(super) fn cuda_input(&self, material: &GemMaterial) -> ToneInput {
        let scale = f64::from(material.absorption_path_scale.max(0.0));
        let principal = self.rays.is_some();
        let tensor = &material.absorption;
        let channels = if principal {
            1
        } else if tensor.beta_ray.is_some() {
            3
        } else if tensor.is_pleochroic {
            2
        } else {
            1
        };
        if self.weight_sum <= 0.0 {
            return ToneInput {
                paths: Vec::new(),
                alpha: vec![[0.0; 3]; WAVELENGTHS],
                principal,
                channels,
                scale,
                denominator: 0.0,
            };
        }
        let alpha = (380..=780)
            .map(|wavelength| principal_alphas(tensor, f64::from(wavelength)))
            .collect();
        let (paths, denominator) = self.rays.as_ref().map_or_else(
            || {
                let weight_sum = f64::from(self.weight_sum);
                let paths = self
                    .bins
                    .iter()
                    .enumerate()
                    .filter(|&(_, &weight)| weight > 0.0)
                    .map(|(bin, &weight)| {
                        let centre = (bin as f32 + 0.5) * self.bin_width;
                        [
                            f64::from(weight) / weight_sum,
                            f64::from(centre) * scale,
                            0.0,
                            0.0,
                        ]
                    })
                    .collect();
                (paths, 1.0)
            },
            |rays| {
                let paths = rays
                    .iter()
                    .map(|ray| {
                        [
                            f64::from(ray.weight),
                            f64::from(ray.principal_path[0]),
                            f64::from(ray.principal_path[1]),
                            f64::from(ray.principal_path[2]),
                        ]
                    })
                    .collect();
                let denominator = rays
                    .iter()
                    .fold(0.0, |sum, ray| sum + f64::from(ray.weight));
                (paths, denominator)
            },
        );
        ToneInput {
            paths,
            alpha,
            principal,
            channels,
            scale,
            denominator,
        }
    }

    /// Finish the returned CUDA spectrum with the exact existing illuminant/CIE transform.
    /// The caller propagates GPU errors before invoking this; there is no CPU spectral retry.
    pub(super) fn finish_from_spectrum(
        &self,
        spectrum: &ToneSpectrum,
        illuminant: ToneIlluminant,
    ) -> FaceUpTone {
        let returned_fraction = self.returned as f32 / self.total.max(1) as f32;
        if self.weight_sum <= 0.0 {
            return FaceUpTone {
                returned_fraction,
                ..FaceUpTone::NONE
            };
        }
        let colour = body_color_from_spectra(
            |wavelength| spectrum[wavelength as usize - 380],
            |wavelength| illuminant.spectral_power(wavelength),
            illuminant.display_adaptation(),
        );
        let [l, a, b] = colour.lab;
        let chroma = a.hypot(b);
        let hue_deg = if chroma < 0.5 {
            0.0
        } else {
            let degrees = b.atan2(a).to_degrees();
            if degrees < 0.0 {
                degrees + 360.0
            } else {
                degrees
            }
        };
        FaceUpTone {
            l_star: l as f32,
            chroma: chroma as f32,
            hue_deg: hue_deg as f32,
            srgb: colour.srgb,
            mean_path_units: self.weighted_path / self.weight_sum,
            returned_fraction,
        }
    }

    /// Turns the histogram (or, for a pleochroic material, the per-ray list) into the tone of
    /// `material` under `illuminant`.
    #[cfg(test)]
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
#[cfg(test)]
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
mod cuda_input_tests {
    use super::*;

    /// Executes the prepared CUDA expressions with CPU exp, isolating packing/order from
    /// backend transcendental differences. This does not initialize or launch a GPU.
    fn prepared_spectrum(input: &ToneInput) -> ToneSpectrum {
        std::array::from_fn(|wave| {
            let alpha = input.alpha[wave];
            let mixture = |channel: usize| {
                input.paths.iter().fold(0.0, |sum, path| {
                    let depth = if input.principal {
                        input.scale
                            * alpha[2]
                                .mul_add(path[3], alpha[1].mul_add(path[2], alpha[0] * path[1]))
                    } else {
                        alpha[if input.channels == 2 && channel == 1 {
                            2
                        } else {
                            channel
                        }] * path[1]
                    };
                    path[0].mul_add((-depth).exp(), sum)
                })
            };
            if input.principal {
                if input.denominator > 0.0 {
                    mixture(0) / input.denominator
                } else {
                    0.0
                }
            } else {
                match input.channels {
                    1 => mixture(0),
                    2 => 2.0f64.mul_add(mixture(0), mixture(1)) / 3.0,
                    _ => (mixture(0) + mixture(1) + mixture(2)) / 3.0,
                }
            }
        })
    }

    fn material() -> GemMaterial {
        let mut material = GemMaterial::sapphire();
        material.absorption = AbsorptionTensor {
            o_ray: vec![AbsorptionBand::new(455.0, 31.0, 0.7)],
            e_ray: vec![AbsorptionBand::energy(615.0, 1800.0, 0.5)],
            beta_ray: Some(vec![AbsorptionBand::new(535.0, 45.0, 0.3)]),
            is_pleochroic: true,
        };
        material.absorption_path_scale = 1.37;
        material
    }

    #[test]
    fn packed_principal_paths_preserve_f64_denominator_and_reference_colour() {
        let mut tone = ToneAccumulator::with_principal_paths(0.8);
        for (weight, path) in [
            (0.1, [0.25, 0.5, 0.75]),
            (0.7, [1.1, 0.2, 0.3]),
            (0.2, [0.1, 0.6, 1.3]),
        ] {
            tone.count_total();
            tone.offer_principal(weight, path.iter().sum(), path);
        }
        let material = material();
        let input = tone.cuda_input(&material);
        let expected = tone
            .principal_rays()
            .iter()
            .fold(0.0, |sum, ray| sum + f64::from(ray.weight));
        assert_eq!(input.denominator.to_bits(), expected.to_bits());
        let illuminant = ToneIlluminant {
            preset: pleochroma::optics::raytracer::LightingPreset::Daylight,
            fallback_to_d65: false,
        };
        assert_eq!(
            tone.finish(&material, illuminant),
            tone.finish_from_spectrum(&prepared_spectrum(&input), illuminant)
        );
    }

    #[test]
    fn packed_bins_preserve_all_three_tensor_mixture_modes() {
        let mut tone = ToneAccumulator::new(1.0);
        for (weight, path) in [(0.17, 0.2), (0.31, 3.0), (0.43, 0.21), (0.09, 40.0)] {
            tone.count_total();
            tone.offer(weight, path);
        }
        for (pleochroic, beta) in [(false, false), (true, false), (false, true)] {
            let mut material = material();
            material.absorption.is_pleochroic = pleochroic;
            if !beta {
                material.absorption.beta_ray = None;
            }
            let input = tone.cuda_input(&material);
            let illuminant = ToneIlluminant {
                preset: pleochroma::optics::raytracer::LightingPreset::Incandescent,
                fallback_to_d65: false,
            };
            assert_eq!(
                tone.finish(&material, illuminant),
                tone.finish_from_spectrum(&prepared_spectrum(&input), illuminant)
            );
        }
    }
}
