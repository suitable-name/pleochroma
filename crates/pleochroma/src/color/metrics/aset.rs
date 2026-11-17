//! Zones (ASET-style) shares and the Contrast figure of one face-up pose.
//!
//! Every traced ray that hits the stone gets five weights that sum to one (to one half per
//! eigenmode for an anisotropic stone): red, green and blue by the direction it leaves the stone
//! (the amplitudes of the Contrast view's rig, [`aset_zone_amplitudes`]), leak when it leaves
//! through a pavilion facet, lost when it never leaves upward within the bounce budget. The
//! shares are the weight sums as percentages of the rays that hit, so `leak_pct` carries the
//! same bits as the Windowing figure.
//!
//! Contrast is the mean absolute difference of the per-spot Blue share between neighbouring spots
//! of the 18 x 18 metrics grid, times 100 (see [`pattern_contrast`]). Only Blue counts as shadow;
//! leak and lost are reported on their own.

use crate::optics::raytracer::environment::aset_zone_amplitudes;
use glam::Vec3;

/// The side of the metrics grid; shared with the grid of the face-up pass.
pub const ASET_GRID: usize = 18;

/// The number of spots of the metrics grid, `ASET_GRID` squared.
const SPOTS: usize = ASET_GRID * ASET_GRID;

/// The ASET zone weights of one traced ray; they sum to one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AsetZoneWeights {
    /// Weight of the red zone, 45 to 75 degrees above the girdle plane.
    pub red: f32,
    /// Weight of the green zone, 0 to 45 degrees above the girdle plane.
    pub green: f32,
    /// Weight of the blue zone, 75 to 90 degrees above the girdle plane.
    pub blue: f32,
    /// Weight of light that left through a pavilion facet (the Windowing rays).
    pub leak: f32,
    /// Weight of light that did not leave upward within the bounce budget.
    pub lost: f32,
}

impl AsetZoneWeights {
    /// A ray that left through a pavilion facet: all weight in leak.
    pub const LEAK: Self = Self {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        leak: 1.0,
        lost: 0.0,
    };

    /// A ray that found no exit within the bounce budget, or left sideways: all weight in lost.
    pub const LOST: Self = Self {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        leak: 0.0,
        lost: 1.0,
    };

    /// The weights of a ray that left the stone along unit `exit_dir`: the rig's amplitudes
    /// ([`aset_zone_amplitudes`]), with the part below the horizon counted as leak.
    #[must_use]
    pub fn from_exit(exit_dir: Vec3) -> Self {
        let [red, green, blue, below] = aset_zone_amplitudes(exit_dir);
        Self {
            red,
            green,
            blue,
            leak: below,
            lost: 0.0,
        }
    }
}

/// ASET-style area shares of one pose, in percent of the rays that hit the stone, and Contrast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AsetAreaShares {
    /// Red zone, 45 to 75 degrees above the girdle plane.
    pub red_pct: f32,
    /// Green zone, 0 to 45 degrees above the girdle plane.
    pub green_pct: f32,
    /// Blue zone, 75 to 90 degrees above the girdle plane.
    pub blue_pct: f32,
    /// Light that left through the pavilion; the same value as the Windowing figure.
    pub leak_pct: f32,
    /// Light with no exit within the bounce budget, or that left sideways.
    pub lost_pct: f32,
    /// The pattern Contrast, 0 to 100 ([`pattern_contrast`]).
    pub contrast: f32,
}

impl AsetAreaShares {
    /// The Dark share: leak plus lost.
    #[must_use]
    pub fn dark_pct(&self) -> f32 {
        self.leak_pct + self.lost_pct
    }
}

/// The Blue share of every grid spot, row-major (`ix * ASET_GRID + iz`); `None` off the stone.
///
/// This is what Contrast is computed from; the command line prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct AsetSpotGrid {
    /// The per-spot Blue share, 0 to 1, or `None` for a spot no ray of which hit the stone.
    pub blue: [Option<f32>; SPOTS],
}

/// The mean difference of the per-spot Blue shares between neighbouring spots, 0 to 100.
///
/// The pairs are every spot with its right neighbour (`ix + 1`) and its lower neighbour
/// (`iz + 1`) on the grid, both on the stone (`Some`). The result is
/// `100 * sum |b_i - b_j| / pairs`, summed in `f64` in row-major order (right pair first, then
/// lower pair), and 0 when there is no pair. Shares outside 0 to 1 are clamped, so the result lies
/// in 0 to 100.
#[must_use]
pub fn pattern_contrast(spots: &[Option<f32>; SPOTS]) -> f32 {
    let mut sum = 0.0_f64;
    let mut pairs = 0_u32;
    for ix in 0..ASET_GRID {
        for iz in 0..ASET_GRID {
            let index = ix * ASET_GRID + iz;
            let Some(here) = spots[index] else {
                continue;
            };
            if ix + 1 < ASET_GRID
                && let Some(right) = spots[index + ASET_GRID]
            {
                sum += pair_difference(here, right);
                pairs += 1;
            }
            if iz + 1 < ASET_GRID
                && let Some(lower) = spots[index + 1]
            {
                sum += pair_difference(here, lower);
                pairs += 1;
            }
        }
    }
    if pairs == 0 {
        return 0.0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the result is in 0..=100; narrowing to f32 is the figure's precision"
    )]
    let contrast = (100.0 * sum / f64::from(pairs)) as f32;
    contrast.clamp(0.0, 100.0)
}

/// The absolute difference of two Blue shares, each clamped to 0 to 1, in `f64`.
fn pair_difference(a: f32, b: f32) -> f64 {
    (f64::from(a.clamp(0.0, 1.0)) - f64::from(b.clamp(0.0, 1.0))).abs()
}

/// The sums of one pose's grid pass: the five zone weights over all rays and, per spot, the Blue
/// weight and the hit weight Contrast is built from.
pub(super) struct AsetAccumulator {
    /// Red, green, blue, leak and lost weight, mode-weighted, over every added ray.
    sums: [f64; 5],
    /// Per spot, the blue weight times the mode weight, summed.
    spot_blue: [f32; SPOTS],
    /// Per spot, the mode weight of every added ray, summed.
    spot_hits: [f32; SPOTS],
}

impl Default for AsetAccumulator {
    /// An empty accumulator; written by hand because arrays of 324 have no derived `Default`.
    fn default() -> Self {
        Self {
            sums: [0.0; 5],
            spot_blue: [0.0; SPOTS],
            spot_hits: [0.0; SPOTS],
        }
    }
}

impl AsetAccumulator {
    /// Folds one ray into spot `spot` (`ix * ASET_GRID + iz`, below 324) with its zone `weights`
    /// and `mode_weight` (1 for an isotropic stone, one half per eigenmode otherwise).
    #[expect(
        clippy::suboptimal_flops,
        reason = "bit-exact pin: `leak_pct` must equal the Windowing percentage bit for bit, and the golden tests pin both"
    )]
    pub(super) fn add(&mut self, spot: usize, weights: AsetZoneWeights, mode_weight: f32) {
        let mode = f64::from(mode_weight);
        self.sums[0] += f64::from(weights.red) * mode;
        self.sums[1] += f64::from(weights.green) * mode;
        self.sums[2] += f64::from(weights.blue) * mode;
        self.sums[3] += f64::from(weights.leak) * mode;
        self.sums[4] += f64::from(weights.lost) * mode;
        self.spot_blue[spot] += weights.blue * mode_weight;
        self.spot_hits[spot] += mode_weight;
    }

    /// The Blue share of every spot: blue weight over hit weight, `None` where nothing hit.
    pub(super) fn spots(&self) -> AsetSpotGrid {
        let mut blue = [None; SPOTS];
        for (share, (sum, hits)) in blue
            .iter_mut()
            .zip(self.spot_blue.iter().zip(&self.spot_hits))
        {
            if *hits > 0.0 {
                *share = Some((sum / hits).clamp(0.0, 1.0));
            }
        }
        AsetSpotGrid { blue }
    }

    /// The shares as percentages of `n_total`, the total ray weight of the pose, and the Contrast
    /// of the spot grid.
    ///
    /// Each percentage is `(sum as f32 / n_total * 100).clamp(0, 100)`, the expression the
    /// Windowing percentage uses, so `leak_pct` and `windowing_pct` are the same bits.
    pub(super) fn finish(&self, n_total: f32) -> AsetAreaShares {
        let n_total = n_total.max(1.0);
        let share = |sum: f64| -> f32 {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the sums are exact multiples of one half or float-precision weights"
            )]
            let sum = sum as f32;
            (sum / n_total * 100.0).clamp(0.0, 100.0)
        };
        AsetAreaShares {
            red_pct: share(self.sums[0]),
            green_pct: share(self.sums[1]),
            blue_pct: share(self.sums[2]),
            leak_pct: share(self.sums[3]),
            lost_pct: share(self.sums[4]),
            contrast: pattern_contrast(&self.spots().blue),
        }
    }
}
