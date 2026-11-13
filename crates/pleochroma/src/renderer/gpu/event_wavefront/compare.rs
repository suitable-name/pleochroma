//! Pixel-wise comparison of two accumulated frames, used by the event wavefront vs megakernel
//! equivalence check and the other GPU twins.
//!
//! Two kernels that run the same WGSL expressions on the same inputs agree at one of three tiers
//! ([`FrameDiff::twin_tier`]), strictest first:
//!
//! - **exact**: no pixel differs in its bits;
//! - **golden**: at most `max(2, 0.1 %)` of the pixels beyond
//!   `1e-5 * max(|a|, |b|) + 1e-6 * spp` ([`FrameDiff::tol_passed`]);
//! - **rounding**: the driver compiled the two kernels with different floating-point contraction
//!   (WGSL cannot forbid it). See [`FrameDiff::ROUNDING_RELATIVE_BOUND`] for the bound and
//!   [`FrameDiff::rounding_passed`] for the rule.
//!
//! Pure CPU code: testable without a GPU. NaN or Inf in either frame is always a hard
//! failure (`NaN > tol` is false, so a naive threshold test would let it through).

use glam::Vec3;

/// The tier two frames agree at, strictest first; see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TwinTier {
    /// No pixel differs in its bits.
    Exact,
    /// The golden rule holds ([`FrameDiff::tol_passed`]).
    Golden,
    /// Only compiler rounding separates the frames ([`FrameDiff::rounding_passed`]).
    Rounding,
    /// The frames differ by more than rounding, or hold a non-finite value.
    Failed,
}

/// Outcome of comparing two frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDiff {
    /// Pixels with at least one bitwise-different channel.
    pub differing_pixels: usize,
    /// Largest absolute channel difference; `INFINITY` when any value is non-finite.
    pub max_abs_diff: f32,
    /// Pixels with a channel beyond `1e-5 * max(|a|, |b|) + 1e-6 * spp`, or non-finite.
    pub tol_failing_pixels: usize,
    /// Pixels where either frame holds a NaN or infinite channel.
    pub non_finite_pixels: usize,
    /// Pixels with a channel beyond
    /// `ROUNDING_RELATIVE_BOUND * (max(|a|, |b|) + frame level)`, or non-finite. The frame level
    /// is the mean over the pixels of their largest absolute channel in either frame.
    pub rounding_failing_pixels: usize,
}

impl FrameDiff {
    /// The largest relative change compiler rounding alone makes to a channel at one event.
    ///
    /// Two compilations of `cos_t = sqrt(1 - eta^2 * (1 - cos_i^2))` may round the radicand
    /// differently by half an ulp of 1 (`2^-24`): one fuses the multiply into the subtraction,
    /// the other does not. Next to the channel's critical angle the radicand is near zero, so
    /// `cos_t` moves by up to `sqrt(2^-24) = 2.44e-4`, and the Fresnel reflectance of the p
    /// polarisation (`1 - 4 n cos_t / cos_i` there) by up to `4 * n * 2.44e-4 / cos_i`: 2.6e-3
    /// for diamond and 3.0e-3 for the largest index of the catalogue (rutile's extraordinary
    /// ray, 2.9, whose critical angle has `cos_i = 0.94`). This is that value rounded up.
    ///
    /// A pixel is held against it relative to its own value plus the frame's level: a grazing
    /// exit can change a nearly black pixel by a large factor, and what bounds that change is
    /// the light of the frame, not the pixel's own value.
    pub const ROUNDING_RELATIVE_BOUND: f32 = 4.0e-3;

    /// Pixels allowed to fail the tolerance: `max(2, 0.1 %)` of `total_pixels`.
    #[must_use]
    pub fn allowed_tol_failing(total_pixels: usize) -> usize {
        2.max(total_pixels / 1000)
    }

    /// Whether the frames pass the TOL tier: no non-finite value anywhere and at most
    /// `max(2, 0.1 %)` pixels beyond tolerance.
    #[must_use]
    pub fn tol_passed(&self, total_pixels: usize) -> bool {
        self.non_finite_pixels == 0
            && self.tol_failing_pixels <= Self::allowed_tol_failing(total_pixels)
    }

    /// Pixels the rounding tier allows beyond the golden tolerance: 20 % of `total_pixels`.
    ///
    /// A rounding difference shows only in the samples that pass close to a critical angle,
    /// a few percent of them. Measured on an NVIDIA A100 (Vulkan): 0.7 % of the pixels at 2
    /// samples and 2.3 to 3.2 % at 4 samples for isotropic stones, 1.9 % at 2 samples for
    /// uniaxial stones. A factor one twin lacks moves every pixel.
    #[must_use]
    pub const fn allowed_rounding_tol_failing(total_pixels: usize) -> usize {
        total_pixels / 5
    }

    /// Pixels the rounding tier allows beyond [`Self::ROUNDING_RELATIVE_BOUND`]:
    /// `max(4, 2 %)` of `total_pixels`.
    ///
    /// These are the samples in which rounding tipped a decision: above all a companion
    /// channel's direction match at an entry, which ends the channel and narrows its family, so
    /// the whole sample changes. Measured on an NVIDIA A100 (Vulkan), megakernel against event
    /// wavefront, with the match evaluated as a cosine: 3 to 16 of 1024 pixels at 2 samples for
    /// isotropic and biaxial stones, none for uniaxial stones.
    #[must_use]
    pub fn allowed_rounding_outliers(total_pixels: usize) -> usize {
        4.max(total_pixels / 50)
    }

    /// Whether only compiler rounding separates the frames: no non-finite value, at most 20 %
    /// of the pixels beyond the golden tolerance and at most `max(4, 2 %)` beyond the rounding
    /// bound.
    #[must_use]
    pub fn rounding_passed(&self, total_pixels: usize) -> bool {
        self.non_finite_pixels == 0
            && self.tol_failing_pixels <= Self::allowed_rounding_tol_failing(total_pixels)
            && self.rounding_failing_pixels <= Self::allowed_rounding_outliers(total_pixels)
    }

    /// The strictest tier the frames agree at.
    #[must_use]
    pub fn twin_tier(&self, total_pixels: usize) -> TwinTier {
        if self.non_finite_pixels > 0 {
            TwinTier::Failed
        } else if self.differing_pixels == 0 {
            TwinTier::Exact
        } else if self.tol_passed(total_pixels) {
            TwinTier::Golden
        } else if self.rounding_passed(total_pixels) {
            TwinTier::Rounding
        } else {
            TwinTier::Failed
        }
    }

    /// Whether two kernels that run the same expressions agree: any tier but
    /// [`TwinTier::Failed`].
    #[must_use]
    pub fn twin_passed(&self, total_pixels: usize) -> bool {
        self.twin_tier(total_pixels) != TwinTier::Failed
    }
}

/// The mean, over the pixels that are finite in both frames, of the pixel's largest absolute
/// channel in either frame; 0 when there is no such pixel.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the mean of f32 values is in f32 range"
)]
fn frame_level(a: &[Vec3], b: &[Vec3]) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for (pa, pb) in a.iter().zip(b) {
        if pa.is_finite() && pb.is_finite() {
            sum += f64::from(pa.abs().max_element().max(pb.abs().max_element()));
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    (sum / count as f64) as f32
}

/// Compares `a` against `b` (same length) for a pass of `spp` samples per pixel.
///
/// # Panics
///
/// Panics if the slices differ in length.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the §7.2 golden tolerance is defined with separately rounded operations; a fused form could flip a borderline pixel"
)]
pub fn compare_frames(a: &[Vec3], b: &[Vec3], spp: u32) -> FrameDiff {
    assert_eq!(a.len(), b.len(), "frames must have the same pixel count");
    let spp_tol = 1e-6 * spp as f32;
    let level = frame_level(a, b);
    let mut diff = FrameDiff {
        differing_pixels: 0,
        max_abs_diff: 0.0,
        tol_failing_pixels: 0,
        non_finite_pixels: 0,
        rounding_failing_pixels: 0,
    };
    for (pa, pb) in a.iter().zip(b) {
        if !pa.is_finite() || !pb.is_finite() {
            diff.non_finite_pixels += 1;
            diff.differing_pixels += 1;
            diff.tol_failing_pixels += 1;
            diff.rounding_failing_pixels += 1;
            diff.max_abs_diff = f32::INFINITY;
            continue;
        }
        let mut bit_diff = false;
        let mut tol_fail = false;
        let mut rounding_fail = false;
        for c in 0..3 {
            let (va, vb) = (pa[c], pb[c]);
            if va.to_bits() != vb.to_bits() {
                bit_diff = true;
                let d = (va - vb).abs();
                diff.max_abs_diff = diff.max_abs_diff.max(d);
                let largest = va.abs().max(vb.abs());
                if d > 1e-5 * largest + spp_tol {
                    tol_fail = true;
                }
                if d > FrameDiff::ROUNDING_RELATIVE_BOUND * (largest + level) {
                    rounding_fail = true;
                }
            }
        }
        diff.differing_pixels += usize::from(bit_diff);
        diff.tol_failing_pixels += usize::from(tol_fail);
        diff.rounding_failing_pixels += usize::from(rounding_fail);
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(n: usize, v: f32) -> Vec<Vec3> {
        vec![Vec3::splat(v); n]
    }

    #[test]
    fn identical_frames_are_exact() {
        let a = flat(100, 2.0);
        let d = compare_frames(&a, &a.clone(), 4);
        assert_eq!(d.differing_pixels, 0);
        assert!(d.tol_passed(100));
        assert_eq!(d.max_abs_diff, 0.0);
        assert_eq!(d.twin_tier(100), TwinTier::Exact);
    }

    #[test]
    fn one_ulp_is_a_tol_pass() {
        let a = flat(100, 2.0);
        let mut b = a.clone();
        b[7].x = f32::from_bits(b[7].x.to_bits() + 1);
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.differing_pixels, 1);
        assert_eq!(d.tol_failing_pixels, 0);
        assert!(d.tol_passed(100));
        assert_eq!(d.twin_tier(100), TwinTier::Golden);
    }

    #[test]
    fn many_large_differences_fail_the_tol_tier() {
        let a = flat(100, 2.0);
        let mut b = a.clone();
        for p in b.iter_mut().take(10) {
            *p = Vec3::splat(3.0);
        }
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.tol_failing_pixels, 10);
        assert!(!d.tol_passed(100));
        // Ten of a hundred pixels off by half their value is no rounding either.
        assert_eq!(d.rounding_failing_pixels, 10);
        assert_eq!(d.twin_tier(100), TwinTier::Failed);
    }

    #[test]
    fn a_couple_of_flips_still_pass() {
        let a = flat(100, 2.0);
        let mut b = a.clone();
        b[0] = Vec3::splat(3.0);
        b[1] = Vec3::splat(3.0);
        assert!(compare_frames(&a, &b, 4).tol_passed(100));
    }

    #[test]
    fn nan_is_never_tol() {
        let a = flat(1_000_000, 1.0);
        let mut b = a.clone();
        b[10].y = f32::NAN;
        let d = compare_frames(&a, &b, 2);
        assert_eq!(d.non_finite_pixels, 1);
        assert!(d.max_abs_diff.is_infinite());
        assert!(!d.tol_passed(1_000_000));
        assert_eq!(d.twin_tier(1_000_000), TwinTier::Failed);
        // NaN in the reference frame is just as fatal.
        assert!(!compare_frames(&b, &a, 2).tol_passed(1_000_000));
    }

    #[test]
    fn infinity_is_never_tol() {
        let a = flat(64, 1.0);
        let mut b = a.clone();
        b[3].z = f32::INFINITY;
        let d = compare_frames(&a, &b, 2);
        assert!(!d.tol_passed(64));
        assert!(!d.twin_passed(64));
    }

    #[test]
    fn identical_nan_frames_still_fail() {
        let mut a = flat(64, 1.0);
        a[0].x = f32::NAN;
        let d = compare_frames(&a, &a.clone(), 2);
        assert!(!d.tol_passed(64));
        assert!(!d.twin_passed(64));
    }

    #[test]
    fn a_few_percent_of_small_differences_are_the_rounding_tier() {
        // 5 % of the pixels off by 1e-4 relative: beyond golden, within the rounding bound.
        let a = flat(1000, 2.0);
        let mut b = a.clone();
        for p in b.iter_mut().take(50) {
            *p = Vec3::splat(2.0002);
        }
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.tol_failing_pixels, 50);
        assert_eq!(d.rounding_failing_pixels, 0);
        assert!(!d.tol_passed(1000));
        assert_eq!(d.twin_tier(1000), TwinTier::Rounding);
        assert!(d.twin_passed(1000));
    }

    #[test]
    fn a_systematic_small_difference_fails_every_tier() {
        // Every pixel off by 1e-4 relative: a factor one twin lacks, never rounding.
        let a = flat(1000, 2.0);
        let b = flat(1000, 2.0002);
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.tol_failing_pixels, 1000);
        assert_eq!(d.twin_tier(1000), TwinTier::Failed);
    }

    #[test]
    fn a_handful_of_flipped_pixels_is_still_the_rounding_tier() {
        // 1 % of the pixels with a tipped random decision: more than the golden rule's two,
        // within the rounding tier's 2 %.
        let a = flat(1000, 2.0);
        let mut b = a.clone();
        for p in b.iter_mut().take(10) {
            *p = Vec3::splat(3.0);
        }
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.rounding_failing_pixels, 10);
        assert!(!d.tol_passed(1000));
        assert_eq!(d.twin_tier(1000), TwinTier::Rounding);
    }

    #[test]
    fn large_differences_in_many_pixels_fail_the_rounding_tier() {
        // 5 % of the pixels off by half their value.
        let a = flat(1000, 2.0);
        let mut b = a.clone();
        for p in b.iter_mut().take(50) {
            *p = Vec3::splat(3.0);
        }
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.rounding_failing_pixels, 50);
        assert_eq!(d.twin_tier(1000), TwinTier::Failed);
    }

    #[test]
    fn a_dark_pixel_is_judged_against_the_frame_level() {
        // A grazing exit changes a nearly black pixel by a factor of three; the frame's level
        // bounds it, not the pixel's own value.
        let mut a = flat(100, 2.0);
        let mut b = a.clone();
        a[3] = Vec3::splat(1e-4);
        b[3] = Vec3::splat(3e-4);
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.tol_failing_pixels, 1);
        assert_eq!(d.rounding_failing_pixels, 0);
    }
}
