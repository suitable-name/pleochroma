//! The refractive-index range a design works in, traced at the angles it is cut with.
//!
//! Every bound is for the design as cut: the same facet angles, no retarget. For each index
//! the stone is traced as a flat material (non-dispersive, non-absorbing, isotropic) at the
//! table-up pose and at the eight small-tilt poses of
//! [`pleochroma::color::metrics::small_tilt`]. The low bound is where windowing starts to
//! rise past what the design shows at its own index; the high bound is where the tilted
//! brilliance falls away or extinction climbs. Whether the high bound is shown or only the
//! low one is decided by [`HIGH_BOUND_CALIBRATED`].

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::atomic::{AtomicBool, Ordering},
};

use pleochroma::{
    color::metrics::{
        FinishedStone, GemOpticalMetrics, evaluate_gem_optical_metrics_finished_core,
        finishes_hash,
        small_tilt::{SMALL_TILT_POSE_COUNT, small_tilt_camera, small_tilt_mean, small_tilt_poses},
    },
    optics::{LightingPreset, materials::GemMaterial, raytracer::EnvironmentSource},
    render_setup::hash_geometry,
};

/// The version of the thresholds below; part of the cache key, so a change recomputes every
/// stored range.
pub const THRESHOLD_VERSION: u32 = 1;

/// Whether the high bound is trusted enough to be shown.
///
/// `false`: the app and the sheet say "N and up", the chart draws the high end dashed and marked
/// "uncalibrated", and material lists judge by the low bound only. `true`: the range reads "N to
/// M" and lists judge by both bounds. The calibration checkpoint sets it.
pub const HIGH_BOUND_CALIBRATED: bool = false;

/// The tilt, in degrees, of the small-tilt poses.
pub const SMALL_TILT_DEG: f32 = 10.0;

/// Windowing, in percent, that never counts against an index however the design does at its own.
pub const WINDOWING_LIMIT_PCT: f32 = 15.0;

/// Percentage points of windowing, above the design's own value, still allowed at a lower index.
pub const WINDOWING_MARGIN_POINTS: f32 = 5.0;

/// Percentage points of tilted brilliance, below the design's own value, still allowed at a
/// higher index.
pub const BRILLIANCE_MARGIN_POINTS: f32 = 10.0;

/// Extinction, in percent, that never counts against an index however the design does at its own.
pub const EXTINCTION_LIMIT_PCT: f32 = 40.0;

/// Percentage points of extinction, above the design's own value, still allowed at a higher
/// index.
pub const EXTINCTION_MARGIN_POINTS: f32 = 10.0;

/// First index of the grid, in hundredths.
const GRID_FIRST_HUNDREDTHS: u32 = 140;
/// Spacing of the grid, in hundredths.
const GRID_STEP_HUNDREDTHS: u32 = 5;
/// Number of grid indices: 1.40 to 2.70 in steps of 0.05.
const GRID_STEPS: u32 = 27;
/// The resolution a bound is bisected to.
const BISECT_RESOLUTION: f64 = 0.01;
/// Slack for comparing indices that went through floating-point arithmetic.
const EPS: f64 = 1e-9;

/// One index step; every figure in percent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RiGridRow {
    /// The refractive index of the flat material.
    pub n: f64,
    /// Windowing with the table up.
    pub windowing_table_up: f32,
    /// Windowing, the mean of the eight small-tilt poses.
    pub windowing_tilt: f32,
    /// Brilliance, the mean of the eight small-tilt poses.
    pub brilliance_tilt: f32,
    /// Extinction, the mean of the eight small-tilt poses.
    pub extinction_tilt: f32,
}

/// The traced range of a design.
#[derive(Debug, Clone, PartialEq)]
pub struct RiRange {
    /// The design's own refractive index.
    pub n0: f64,
    /// The design at its own index.
    pub own: RiGridRow,
    /// Ascending `n`: the 0.05 grid from 1.40 to 2.70 plus the bisection points.
    pub rows: Vec<RiGridRow>,
    /// The lowest index that works, rounded up to 0.01; `None` for a stone with no geometry.
    pub n_low: Option<f64>,
    /// The highest index that works, rounded down to 0.01; `None` when none of the grid up to
    /// 2.70 fails.
    pub n_high: Option<f64>,
    /// [`THRESHOLD_VERSION`] when the range was traced.
    pub threshold_version: u32,
}

/// Who stated a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiSource {
    /// The designer, in the file or by hand.
    Designer,
    /// Pleochroma's trace.
    Estimate,
}

/// What a sheet or a chip prints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RiStatement {
    /// The low end.
    pub low: f64,
    /// The high end, when one is stated.
    pub high: Option<f64>,
    /// Who stated it.
    pub source: RiSource,
}

/// What a bound search found.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bound {
    /// The cancel flag was raised.
    Cancelled,
    /// The bound, rounded.
    At(f64),
    /// No index of the grid fails.
    Open,
}

/// The lighting every range is traced under: uniform, so the figures do not move with the
/// lighting a user has chosen.
const fn environment() -> EnvironmentSource<'static> {
    LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95)
}

/// A flat material of index `n`: no dispersion, no absorption, isotropic.
fn flat_material(n: f64) -> GemMaterial {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the material storage precision; a refractive index is in the normal float range"
    )]
    let index = n as f32;
    GemMaterial::new_custom("Flat", index, 0.0, 0.0, [0.0; 3])
}

/// The `i`-th grid index.
fn grid_n(i: u32) -> f64 {
    f64::from(GRID_FIRST_HUNDREDTHS + GRID_STEP_HUNDREDTHS * i) / 100.0
}

/// `value` rounded to the nearest 0.01.
fn round_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// One pose of `stone` in a flat material; `None` when cancelled.
fn measure(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    (yaw, pitch): (f32, f32),
    cancel: &AtomicBool,
) -> Option<GemOpticalMetrics> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some(evaluate_gem_optical_metrics_finished_core(
        stone,
        material,
        yaw,
        pitch,
        environment(),
    ))
}

/// The nine poses of one index: table up, then the eight small-tilt poses. `None` when
/// cancelled between poses.
fn row_at(stone: FinishedStone<'_>, n: f64, cancel: &AtomicBool) -> Option<RiGridRow> {
    let material = flat_material(n);
    let table_up = measure(stone, &material, (0.0, 90.0f32.to_radians()), cancel)?;
    let mut brilliance = [0.0f32; SMALL_TILT_POSE_COUNT];
    let mut extinction = [0.0f32; SMALL_TILT_POSE_COUNT];
    let mut windowing = [0.0f32; SMALL_TILT_POSE_COUNT];
    for (slot, &(azimuth, tilt)) in small_tilt_poses(SMALL_TILT_DEG).iter().enumerate() {
        let metrics = measure(stone, &material, small_tilt_camera(azimuth, tilt), cancel)?;
        brilliance[slot] = metrics.brilliance_pct;
        extinction[slot] = metrics.extinction_pct;
        windowing[slot] = metrics.windowing_pct;
    }
    Some(RiGridRow {
        n,
        windowing_table_up: table_up.windowing_pct,
        windowing_tilt: small_tilt_mean(&windowing),
        brilliance_tilt: small_tilt_mean(&brilliance),
        extinction_tilt: small_tilt_mean(&extinction),
    })
}

/// Whether `row` keeps both windowing limits, given the design's own row.
fn meets_low(row: &RiGridRow, own: &RiGridRow) -> bool {
    let limit = |own_value: f32| WINDOWING_LIMIT_PCT.max(own_value + WINDOWING_MARGIN_POINTS);
    row.windowing_tilt <= limit(own.windowing_tilt)
        && row.windowing_table_up <= limit(own.windowing_table_up)
}

/// Whether `row` keeps tilted brilliance and extinction, given the design's own row.
fn meets_high(row: &RiGridRow, own: &RiGridRow) -> bool {
    row.brilliance_tilt >= own.brilliance_tilt - BRILLIANCE_MARGIN_POINTS
        && row.extinction_tilt
            <= EXTINCTION_LIMIT_PCT.max(own.extinction_tilt + EXTINCTION_MARGIN_POINTS)
}

/// The rows traced so far: every grid index first, then the bisection points.
struct Probe<'a> {
    stone: FinishedStone<'a>,
    cancel: &'a AtomicBool,
    rows: Vec<RiGridRow>,
}

impl Probe<'_> {
    /// The row at `n`: the one already traced, or a new one. `None` when cancelled.
    fn row(&mut self, n: f64) -> Option<RiGridRow> {
        if let Some(found) = self.rows.iter().find(|row| (row.n - n).abs() < EPS) {
            return Some(*found);
        }
        let row = row_at(self.stone, n, self.cancel)?;
        self.rows.push(row);
        Some(row)
    }
}

/// Halves the interval between an index that fails (`bad`) and one that works (`good`), both
/// given in whole hundredths, until they are one hundredth apart; returns the working end in
/// hundredths. `None` when cancelled.
///
/// Every index probed is a whole hundredth, so the result is the first hundredth that works
/// and not a bracket of it.
fn bisect(mut bad: f64, mut good: f64, meets: &mut dyn FnMut(f64) -> Option<bool>) -> Option<f64> {
    while (bad - good).abs() > BISECT_RESOLUTION * 100.0 + 0.5 {
        let mid = good + ((bad - good) / 2.0).trunc();
        if meets(mid / 100.0)? {
            good = mid;
        } else {
            bad = mid;
        }
    }
    Some(good)
}

/// The low bound: walks the grid down from `n0` to the first index that fails, then bisects.
/// `meets` answers whether an index keeps the low limits; `None` is a cancel.
fn search_low(n0: f64, meets: &mut dyn FnMut(f64) -> Option<bool>) -> Bound {
    let mut good = n0;
    for i in (0..GRID_STEPS).rev() {
        let n = grid_n(i);
        if n >= n0 - EPS {
            continue;
        }
        match meets(n) {
            None => return Bound::Cancelled,
            Some(true) => good = n,
            Some(false) => {
                let bad_h = (n * 100.0).round();
                let good_h = (good * 100.0 - EPS).ceil();
                return bisect(bad_h, good_h, meets).map_or(Bound::Cancelled, |found| {
                    // Never above the design's own index.
                    Bound::At((found / 100.0).min(round_hundredths(n0)))
                });
            }
        }
    }
    Bound::At(good.min(round_hundredths(n0)))
}

/// The high bound: walks the grid up from `n0` to the first index that fails, then bisects.
/// `meets` answers whether an index keeps the high limits; `None` is a cancel.
fn search_high(n0: f64, meets: &mut dyn FnMut(f64) -> Option<bool>) -> Bound {
    let mut good = n0;
    for i in 0..GRID_STEPS {
        let n = grid_n(i);
        if n <= n0 + EPS {
            continue;
        }
        match meets(n) {
            None => return Bound::Cancelled,
            Some(true) => good = n,
            Some(false) => {
                let bad_h = (n * 100.0).round();
                let good_h = (good * 100.0 + EPS).floor();
                return bisect(bad_h, good_h, meets).map_or(Bound::Cancelled, |found| {
                    // Never below the design's own index.
                    Bound::At((found / 100.0).max(round_hundredths(n0)))
                });
            }
        }
    }
    Bound::Open
}

/// Traces the range of a design with index `n0`.
///
/// `stone` is the finished-stone argument of
/// `evaluate_gem_optical_metrics_finished_core`. It costs about 350 pose evaluations and checks
/// `cancel` before each of them; a cancel gives `None`. A stone with no planes gives a range with
/// no rows and no bounds.
#[must_use]
pub fn compute(stone: FinishedStone<'_>, n0: f64, cancel: &AtomicBool) -> Option<RiRange> {
    if stone.geom.planes.is_empty() {
        let own = RiGridRow {
            n: n0,
            windowing_table_up: 0.0,
            windowing_tilt: 0.0,
            brilliance_tilt: 0.0,
            extinction_tilt: 0.0,
        };
        return Some(RiRange {
            n0,
            own,
            rows: Vec::new(),
            n_low: None,
            n_high: None,
            threshold_version: THRESHOLD_VERSION,
        });
    }
    let own = row_at(stone, n0, cancel)?;
    let mut probe = Probe {
        stone,
        cancel,
        rows: Vec::new(),
    };
    for i in 0..GRID_STEPS {
        probe.row(grid_n(i))?;
    }
    let low = search_low(n0, &mut |n: f64| {
        probe.row(n).map(|row| meets_low(&row, &own))
    });
    let high = search_high(n0, &mut |n: f64| {
        probe.row(n).map(|row| meets_high(&row, &own))
    });
    let n_low = match low {
        Bound::Cancelled => return None,
        Bound::At(n) => Some(n),
        Bound::Open => None,
    };
    let n_high = match high {
        Bound::Cancelled => return None,
        Bound::At(n) => Some(n),
        Bound::Open => None,
    };
    let mut rows = probe.rows;
    rows.sort_by(|a, b| a.n.total_cmp(&b.n));
    Some(RiRange {
        n0,
        own,
        rows,
        n_low,
        n_high,
        threshold_version: THRESHOLD_VERSION,
    })
}

/// The geometry part of the cache key: the planes, the tools and which facets are frosted.
#[must_use]
pub fn planes_hash(stone: FinishedStone<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_geometry(stone.geom).hash(&mut hasher);
    finishes_hash(stone.finishes).hash(&mut hasher);
    hasher.finish()
}

/// The whole cache key of a stone's range: [`planes_hash`] and [`THRESHOLD_VERSION`].
#[must_use]
pub fn cache_key(stone: FinishedStone<'_>) -> u64 {
    key_for_version(planes_hash(stone), THRESHOLD_VERSION)
}

/// `planes_hash` combined with a threshold version.
fn key_for_version(planes_hash: u64, version: u32) -> u64 {
    let mut hasher = DefaultHasher::new();
    planes_hash.hash(&mut hasher);
    version.hash(&mut hasher);
    hasher.finish()
}

impl RiRange {
    /// The estimate as it is stated: the high end only when `calibrated`.
    ///
    /// `None` when there is no low bound.
    #[must_use]
    pub fn statement_as(&self, calibrated: bool) -> Option<RiStatement> {
        let low = self.n_low?;
        Some(RiStatement {
            low,
            high: if calibrated { self.n_high } else { None },
            source: RiSource::Estimate,
        })
    }

    /// [`Self::statement_as`] with [`HIGH_BOUND_CALIBRATED`].
    #[must_use]
    pub fn statement(&self) -> Option<RiStatement> {
        self.statement_as(HIGH_BOUND_CALIBRATED)
    }

    /// Whether a material of index `n` works: the low bound, and the high one when `calibrated`.
    ///
    /// The index is compared at the 0.01 the bounds are rounded to.
    #[must_use]
    pub fn contains_as(&self, n: f64, calibrated: bool) -> bool {
        let Some(low) = self.n_low else {
            return false;
        };
        let n = round_hundredths(n);
        if n < low - EPS {
            return false;
        }
        !calibrated || self.n_high.is_none_or(|high| n <= high + EPS)
    }

    /// [`Self::contains_as`] with [`HIGH_BOUND_CALIBRATED`].
    #[must_use]
    pub fn contains(&self, n: f64) -> bool {
        self.contains_as(n, HIGH_BOUND_CALIBRATED)
    }

    /// The 10 degree windowing mean at the design's own index, for the windowing Check.
    #[must_use]
    pub const fn own_small_tilt_windowing_pct(&self) -> f32 {
        self.own.windowing_tilt
    }
}

/// "1.63 and up" or "1.63 to 2.31".
#[must_use]
pub fn range_text(statement: &RiStatement) -> String {
    statement.high.map_or_else(
        || format!("{:.2} and up", statement.low),
        |high| format!("{:.2} to {high:.2}", statement.low),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::{StoneGeometry, cuts::StandardGemCuts};

    /// A range with the given bounds and nothing else.
    fn range_with(n_low: Option<f64>, n_high: Option<f64>) -> RiRange {
        let row = RiGridRow {
            n: 1.5,
            windowing_table_up: 3.0,
            windowing_tilt: 4.0,
            brilliance_tilt: 60.0,
            extinction_tilt: 20.0,
        };
        RiRange {
            n0: 1.5,
            own: row,
            rows: vec![row],
            n_low,
            n_high,
            threshold_version: THRESHOLD_VERSION,
        }
    }

    /// A stone whose low limit is known: the index passes from `limit` upward.
    fn low_search(n0: f64, limit: f64) -> Bound {
        search_low(n0, &mut |n: f64| Some(n >= limit))
    }

    /// The low bound lands within 0.01 of a known limit, and a limit between two hundredths is
    /// rounded up, never down.
    #[test]
    fn the_low_bound_is_the_limit_rounded_up() {
        assert_eq!(low_search(2.0, 1.6234), Bound::At(1.63));
        assert_eq!(low_search(2.0, 1.6301), Bound::At(1.64));
        assert_eq!(low_search(1.9, 1.58), Bound::At(1.58));
        assert_eq!(low_search(1.9, 1.5801), Bound::At(1.59));
        assert_eq!(low_search(2.0, 1.40), Bound::At(1.4));
    }

    /// The low bound is never above the design's own index, even for a limit just below it.
    #[test]
    fn the_low_bound_never_passes_the_design_index() {
        let Bound::At(low) = low_search(1.768, 1.7679) else {
            panic!("expected a bound");
        };
        assert!(low <= round_hundredths(1.768) + EPS);
    }

    /// A raised cancel stops the search.
    #[test]
    fn a_cancelled_search_reports_it() {
        assert_eq!(search_low(2.0, &mut |_: f64| None), Bound::Cancelled);
        assert_eq!(search_high(2.0, &mut |_: f64| None), Bound::Cancelled);
    }

    /// The high bound is the last working index rounded down; a stone that never fails has
    /// none.
    #[test]
    fn the_high_bound_is_the_limit_rounded_down() {
        assert_eq!(
            search_high(1.8, &mut |n: f64| Some(n <= 2.3166)),
            Bound::At(2.31)
        );
        assert_eq!(search_high(1.8, &mut |_: f64| Some(true)), Bound::Open);
        let Bound::At(high) = search_high(1.768, &mut |n: f64| Some(n <= 1.7681)) else {
            panic!("expected a bound");
        };
        assert!(high >= round_hundredths(1.768) - EPS);
    }

    /// Both text forms.
    #[test]
    fn the_range_text_has_two_forms() {
        let open = RiStatement {
            low: 1.63,
            high: None,
            source: RiSource::Estimate,
        };
        let closed = RiStatement {
            low: 1.63,
            high: Some(2.31),
            source: RiSource::Designer,
        };
        assert_eq!(range_text(&open), "1.63 and up");
        assert_eq!(range_text(&closed), "1.63 to 2.31");
    }

    /// Both values of the calibration switch give the right statement and the right lists.
    #[test]
    fn both_calibration_forms_state_and_judge_right() {
        let range = range_with(Some(1.63), Some(2.31));
        let uncalibrated = range.statement_as(false);
        let calibrated = range.statement_as(true);
        assert_eq!(
            uncalibrated.map(|s| range_text(&s)),
            Some("1.63 and up".into())
        );
        assert_eq!(
            calibrated.map(|s| range_text(&s)),
            Some("1.63 to 2.31".into())
        );
        assert_eq!(calibrated.map(|s| s.source), Some(RiSource::Estimate));
        // A material above the high bound: out when judged by both, in by the low bound alone.
        assert!(range.contains_as(2.5, false));
        assert!(!range.contains_as(2.5, true));
        assert!(range.contains_as(1.768, true));
        assert!(!range.contains_as(1.60, false));
        assert!(!range.contains_as(1.60, true));
        // No high bound found: both forms accept everything above the low one.
        let open = range_with(Some(1.63), None);
        assert!(open.contains_as(2.6, true));
        assert_eq!(
            open.statement_as(true).map(|s| range_text(&s)),
            Some("1.63 and up".into())
        );
        // No low bound: no statement, nothing works.
        let none = range_with(None, None);
        assert_eq!(none.statement_as(true), None);
        assert!(!none.contains_as(2.0, false));
        // The constant-driven forms follow `HIGH_BOUND_CALIBRATED`.
        assert_eq!(range.statement(), range.statement_as(HIGH_BOUND_CALIBRATED));
        assert_eq!(
            range.contains(2.5),
            range.contains_as(2.5, HIGH_BOUND_CALIBRATED)
        );
    }

    /// The cache key moves with the threshold version and with the geometry.
    #[test]
    fn the_cache_key_carries_the_threshold_version() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let stone = FinishedStone::polished(StoneGeometry::planes_only(&planes));
        let other = FinishedStone::polished(StoneGeometry::planes_only(&emerald));
        let hash = planes_hash(stone);
        assert_ne!(
            key_for_version(hash, THRESHOLD_VERSION),
            key_for_version(hash, THRESHOLD_VERSION + 1)
        );
        assert_eq!(cache_key(stone), key_for_version(hash, THRESHOLD_VERSION));
        assert_eq!(planes_hash(stone), planes_hash(stone));
        assert_ne!(planes_hash(stone), planes_hash(other));
    }

    /// A traced round brilliant: a sorted grid with the bisection points, a low bound that is
    /// 0.01-rounded and works, one hundredth below which the limits fail, and a cancel.
    #[test]
    fn a_traced_stone_has_a_consistent_range() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let stone = FinishedStone::polished(StoneGeometry::planes_only(&planes));
        let never = AtomicBool::new(false);
        let n0 = 2.42;
        let range = compute(stone, n0, &never).expect("not cancelled");
        assert_eq!(range.threshold_version, THRESHOLD_VERSION);
        assert!((range.own.n - n0).abs() < EPS);
        assert!(range.rows.len() >= GRID_STEPS as usize);
        assert!(range.rows.windows(2).all(|pair| pair[0].n < pair[1].n));
        assert!((range.rows[0].n - 1.40).abs() < EPS);
        let last = range.rows.last().expect("rows");
        assert!((last.n - 2.70).abs() < 0.02);
        let low = range.n_low.expect("a low bound");
        assert!((1.40 - EPS..=n0 + EPS).contains(&low));
        assert!((low * 100.0 - (low * 100.0).round()).abs() < 1e-6);
        let at_low = row_at(stone, low, &never).expect("not cancelled");
        assert!(meets_low(&at_low, &range.own));
        let raised = AtomicBool::new(true);
        assert!(compute(stone, n0, &raised).is_none());
    }

    /// A stone with no planes has no bounds and costs nothing.
    #[test]
    fn a_stone_without_planes_has_no_bounds() {
        let stone = FinishedStone::polished(StoneGeometry::planes_only(&[]));
        let range = compute(stone, 1.77, &AtomicBool::new(false)).expect("not cancelled");
        assert!(range.rows.is_empty());
        assert_eq!(range.n_low, None);
        assert_eq!(range.n_high, None);
        assert_eq!(range.statement(), None);
    }
}
