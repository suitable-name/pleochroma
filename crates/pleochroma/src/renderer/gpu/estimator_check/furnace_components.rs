//! Decompose the existing HDR readback without changing or instrumenting the shader.
//! Final XYZ minus CPU integration of the GPU's post-split spectral output estimates
//! effective HG-scatter NEE. Surface NEE is already in the spectral output and
//! remains in the reconstructed term. The residual also includes CPU/GPU integration
//! roundoff and final clamping,
//! so it is a diagnostic residual, not a direct measurement of `nee_xyz`.

use crate::optics::raytracer::integrate_channels_to_xyz_families;
use glam::Vec3;

use super::{TransportDispatch, WelfordXyz};

/// Absolute integration floor, matching the spectral debug comparison.
const RESIDUAL_ABS_FLOOR: f32 = 1e-5;
/// Relative roundoff allowance inspired by the spectral debug budget of 64 ULP.
/// Unlike that check, this uses a conservative absolute-plus-relative allowance
/// (64 f32 epsilons), not exact ULP distances. This never relaxes energy gates.
const RESIDUAL_REL_FLOOR: f32 = 64.0 * f32::EPSILON;

/// A bounded representative tuple, identified in the original global sample space.
#[derive(Debug, Clone)]
pub struct FurnaceComponentSample {
    /// Why this tuple is included (representative, nonfinite, malformed, or negative).
    pub reason: &'static str,
    /// Global pixel index, including the dispatch's pixel offset.
    pub pixel: u64,
    /// Global sample index, including the independent furnace run's sample offset.
    pub sample: u64,
    /// Final GPU XYZ readback.
    pub final_xyz: Vec3,
    /// CPU integration of the GPU's post-split spectral output, if valid. Includes
    /// surface NEE (already deposited spectrally) as well as continuation.
    pub continuation_xyz: Option<Vec3>,
    /// Final minus reconstructed continuation, if valid; includes integration roundoff.
    pub residual_xyz: Option<Vec3>,
}

/// Host-only decomposition of an HDR furnace's already recorded GPU output.
#[derive(Debug, Clone)]
pub struct FurnaceComponents {
    /// Number of tuples requested from the shader.
    pub expected_samples: usize,
    /// Number of tuples successfully decomposed; means cover only these tuples.
    pub valid_samples: usize,
    /// Whether any readback buffer has an unexpected length or layout.
    pub malformed_buffers: bool,
    /// Tuples containing a nonfinite input or reconstructed output.
    pub non_finite_samples: usize,
    /// Tuples with out-of-range, non-reflexive, or asymmetric compatibility masks.
    pub malformed_compat_samples: usize,
    /// Tuples with a negative residual beyond the documented integration allowance.
    /// Informational: the residual is reconstructed, not the shader's raw NEE value.
    pub negative_residual_samples: usize,
    /// Mean final XYZ over successfully decomposed tuples.
    pub final_mean: Vec3,
    /// Mean reconstructed spectral XYZ over successfully decomposed tuples: includes
    /// continuation and any surface NEE, which is deposited directly into radiance.
    pub continuation_mean: Vec3,
    /// Mean effective HG-scatter NEE residual over successfully decomposed tuples.
    pub residual_mean: Vec3,
    /// Absolute allowance used when counting negative residuals.
    pub residual_abs_floor: f32,
    /// Relative allowance used when counting negative residuals.
    pub residual_rel_floor: f32,
    /// At most six tuples: first/middle/last, and first of each diagnostic failure.
    pub samples: Vec<FurnaceComponentSample>,
}

impl FurnaceComponents {
    /// Nonfinite or malformed readback cannot pass an energy anchor. Negative residual
    /// counts remain diagnostic; the original energy and z-score gates are unchanged.
    #[must_use]
    pub const fn valid(&self) -> bool {
        !self.malformed_buffers
            && self.expected_samples > 0
            && self.valid_samples == self.expected_samples
            && self.non_finite_samples == 0
            && self.malformed_compat_samples == 0
    }
}

pub(super) fn decompose(
    dispatch: &TransportDispatch,
    total: usize,
    samples_per_pixel: u32,
    sample_offset: u32,
    pixel_offset: u32,
) -> FurnaceComponents {
    let mut result = FurnaceComponents {
        expected_samples: total,
        valid_samples: 0,
        malformed_buffers: samples_per_pixel == 0
            || total.checked_mul(3) != Some(dispatch.xyz.len())
            || [
                dispatch.radiance.len(),
                dispatch.lambdas.len(),
                dispatch.path_pdf.len(),
                dispatch.compat.len(),
            ]
            .iter()
            .any(|&len| total.checked_mul(8) != Some(len)),
        non_finite_samples: 0,
        malformed_compat_samples: 0,
        negative_residual_samples: 0,
        final_mean: Vec3::ZERO,
        continuation_mean: Vec3::ZERO,
        residual_mean: Vec3::ZERO,
        residual_abs_floor: RESIDUAL_ABS_FLOOR,
        residual_rel_floor: RESIDUAL_REL_FLOOR,
        samples: Vec::new(),
    };
    if result.malformed_buffers {
        return result;
    }
    let (mut final_acc, mut continuation_acc, mut residual_acc) = (
        WelfordXyz::default(),
        WelfordXyz::default(),
        WelfordXyz::default(),
    );
    for idx in 0..total {
        let ReconstructedTuple {
            final_xyz,
            continuation,
            residual,
            malformed,
            non_finite,
        } = reconstruct_tuple(dispatch, idx);
        let first_non_finite = non_finite && result.non_finite_samples == 0;
        let first_malformed = malformed && result.malformed_compat_samples == 0;
        result.non_finite_samples += usize::from(non_finite);
        result.malformed_compat_samples += usize::from(malformed);
        let mut first_negative = false;
        if let (Some(continuation), Some(residual)) = (continuation, residual)
            && !non_finite
        {
            final_acc.update(final_xyz);
            continuation_acc.update(continuation);
            residual_acc.update(residual);
            result.valid_samples += 1;
            let tolerance = Vec3::splat(RESIDUAL_ABS_FLOOR)
                + final_xyz.abs().max(continuation.abs()) * RESIDUAL_REL_FLOOR;
            if residual.cmplt(-tolerance).any() {
                first_negative = result.negative_residual_samples == 0;
                result.negative_residual_samples += 1;
            }
        }
        let reason = if first_non_finite {
            Some("nonfinite")
        } else if first_malformed {
            Some("malformed compatibility")
        } else if first_negative {
            Some("negative residual")
        } else if idx == 0 || idx == total / 2 || idx + 1 == total {
            Some("representative")
        } else {
            None
        };
        if let Some(reason) = reason {
            result.samples.push(FurnaceComponentSample {
                reason,
                pixel: u64::from(pixel_offset) + (idx as u64) / u64::from(samples_per_pixel),
                sample: u64::from(sample_offset) + (idx as u64) % u64::from(samples_per_pixel),
                final_xyz,
                continuation_xyz: continuation,
                residual_xyz: residual,
            });
        }
    }
    result.final_mean = final_acc.mean();
    result.continuation_mean = continuation_acc.mean();
    result.residual_mean = residual_acc.mean();
    result
}

/// Readback lengths have already been validated before any tuple is reconstructed.
struct ReconstructedTuple {
    final_xyz: Vec3,
    continuation: Option<Vec3>,
    residual: Option<Vec3>,
    malformed: bool,
    non_finite: bool,
}

fn reconstruct_tuple(dispatch: &TransportDispatch, idx: usize) -> ReconstructedTuple {
    let final_xyz = Vec3::from_slice(&dispatch.xyz[idx * 3..idx * 3 + 3]);
    let radiance = std::array::from_fn(|k| dispatch.radiance[idx * 8 + k]);
    let lambdas = std::array::from_fn(|k| dispatch.lambdas[idx * 8 + k]);
    let path_pdf = std::array::from_fn(|k| dispatch.path_pdf[idx * 8 + k]);
    let raw_compat = &dispatch.compat[idx * 8..idx * 8 + 8];
    let malformed = !valid_compat(raw_compat);
    let non_finite = !final_xyz.is_finite()
        || radiance
            .iter()
            .chain(&lambdas)
            .chain(&path_pdf)
            .any(|x| !x.is_finite());
    let continuation = if !malformed && !non_finite {
        let compat = std::array::from_fn(|k| {
            u8::try_from(raw_compat[k]).expect("valid_compat checked every mask fits u8")
        });
        // HDR finalization does not white-balance. Its pre-clamp terms are
        // nonnegative; match the shared spectral-debug reconstruction clamp.
        let integrated =
            integrate_channels_to_xyz_families(&radiance, &lambdas, &path_pdf, 0, compat);
        // Do not let `max` replace a NaN with zero before validation.
        Some(if integrated.is_finite() {
            integrated.max(Vec3::ZERO)
        } else {
            integrated
        })
    } else {
        None
    };
    let residual = continuation.map(|value| final_xyz - value);
    let non_finite = non_finite
        || continuation.is_some_and(|v| !v.is_finite())
        || residual.is_some_and(|v| !v.is_finite());
    ReconstructedTuple {
        final_xyz,
        continuation,
        residual,
        malformed,
        non_finite,
    }
}

// `narrow_compat` (transport_physics/08_exit_splitting.wgsl) starts from 0xff
// and only clears symmetric a/b bits for a != b. Self bits must always remain.
fn valid_compat(masks: &[u32]) -> bool {
    masks.iter().enumerate().all(|(a, &mask)| {
        u8::try_from(mask).is_ok()
            && mask & (1 << a) != 0
            && masks
                .iter()
                .enumerate()
                .all(|(b, &other)| (mask & (1 << b) != 0) == (other & (1 << a) != 0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(nee: Vec3) -> TransportDispatch {
        let radiance = [0.5; 8];
        let lambdas = std::array::from_fn(|k| (k as f32).mul_add(40.0, 400.0));
        let path_pdf = [1.0; 8];
        let xyz =
            integrate_channels_to_xyz_families(&radiance, &lambdas, &path_pdf, 0, [255; 8]) + nee;
        TransportDispatch {
            xyz: xyz.to_array().to_vec(),
            radiance: radiance.to_vec(),
            lambdas: lambdas.to_vec(),
            path_pdf: path_pdf.to_vec(),
            compat: vec![255; 8],
        }
    }

    #[test]
    fn decomposition_retains_the_contribution_and_global_replay_coordinates() {
        let nee = Vec3::new(0.1, 0.2, 0.3);
        let one = dispatch(nee);
        let input = TransportDispatch {
            xyz: one.xyz.repeat(4),
            radiance: one.radiance.repeat(4),
            lambdas: one.lambdas.repeat(4),
            path_pdf: one.path_pdf.repeat(4),
            compat: one.compat.repeat(4),
        };
        let result = decompose(&input, 4, 2, 1800, 37);
        assert!(result.valid());
        assert!((result.residual_mean - nee).abs().max_element() < 1e-6);
        assert_eq!(
            (result.samples[0].pixel, result.samples[0].sample),
            (37, 1800)
        );
        assert_eq!(result.negative_residual_samples, 0);
        assert_eq!(
            (result.samples[1].pixel, result.samples[1].sample),
            (38, 1800)
        );
        assert_eq!(
            (result.samples[2].pixel, result.samples[2].sample),
            (38, 1801)
        );
    }

    #[test]
    fn nonfinite_and_malformed_readback_cannot_be_accepted() {
        for channel in 0..8 {
            let mut input = dispatch(Vec3::ZERO);
            input.radiance[channel] = f32::NAN;
            let result = decompose(&input, 1, 600, 600, 0);
            assert!(!result.valid());
            assert_eq!(result.non_finite_samples, 1);
        }
        for invalid in [0, 256, 254, 253] {
            let mut input = dispatch(Vec3::ZERO);
            input.compat[0] = invalid;
            let result = decompose(&input, 1, 600, 600, 0);
            assert!(!result.valid());
            assert_eq!(result.malformed_compat_samples, 1);
        }
        let mut input = dispatch(Vec3::ZERO);
        input.path_pdf.pop();
        assert!(decompose(&input, 1, 600, 600, 0).malformed_buffers);
        // All inputs are finite, but the host's family integral overflows. Do not
        // let the final nonnegative clamp turn that NaN into a plausible zero.
        let mut input = dispatch(Vec3::ZERO);
        input.path_pdf.fill(f32::MAX);
        let result = decompose(&input, 1, 600, 600, 0);
        assert!(!result.valid());
        assert_eq!(result.non_finite_samples, 1);
    }

    #[test]
    fn compatibility_accepts_narrowed_families_without_requiring_transitivity() {
        assert!(valid_compat(&[1, 2, 4, 8, 16, 32, 64, 128]));
        // Pairwise direction agreement need not form a transitive equivalence:
        // channels 0/1 and 1/2 agree while 0/2 do not.
        assert!(valid_compat(&[3, 7, 6, 8, 16, 32, 64, 128]));
    }

    #[test]
    fn reconstruction_roundoff_is_not_reported_as_negative_nee() {
        let small = decompose(&dispatch(Vec3::splat(-1e-6)), 1, 1, 0, 0);
        assert!(small.valid());
        assert_eq!(small.negative_residual_samples, 0);
        let large = decompose(&dispatch(Vec3::splat(-0.1)), 1, 1, 0, 0);
        assert_eq!(large.negative_residual_samples, 1);
    }
}
