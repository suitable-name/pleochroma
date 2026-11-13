//! [`TierTarget`]: authoring-level constraint kinds ("cut to 3.20 mm", "girdle
//! 2% of width", "table 4.10 mm wide") that resolve to a real
//! [`MeetConstraint::ScaleReference`] before solving, instead of a cutter
//! authoring a dimensionless mast directly.
//!
//! # Why these are not [`MeetConstraint`] variants
//!
//! [`MeetConstraint`] is owned by `pleochroma`'s geometry crate (the meet solver),
//! which this crate does not modify. [`TierTarget`] instead lives in
//! [`super::Design::tier_targets`], a `BTreeMap<TierId, TierTarget>` -- the exact
//! same "`Design`-level map, not a [`super::ConstraintTier`] field" pattern
//! [`super::Design::cheater_offsets_deg`] already uses, and for the same reason
//! (widening `ConstraintTier` would be a breaking change to dozens of construction
//! sites outside this crate). Keyed by [`super::TierId`] rather than position, so a
//! target survives `AddTier`/`RemoveTier`/`MoveTier` the same way
//! [`super::Design::tier_ids`] itself does.
//!
//! # Resolution
//!
//! [`Design::resolved_meet_tier_inputs`] is the one place a [`TierTarget`] ever
//! turns into a mast: called from [`super::Design::solve_with`]/
//! [`super::Design::resolve_dirty_with`] in place of the raw
//! [`super::Design::meet_tier_inputs`] whenever [`super::Design::tier_targets`] is
//! non-empty (a no-op, `self.meet_tier_inputs()` unchanged, whenever it is empty --
//! every existing design has an empty map, so this changes nothing for them).
//!
//! - [`TierTarget::DepthMm`] resolves by iterating the bootstrap-and-convert step
//!   to a fixed point: solve with the target tier pinned at the current mast
//!   guess (starting from `0.0`) to measure the design's own `width_axis`,
//!   convert `depth_mm` to a mast via `crate::yield_metrics::mm_per_unit`, and
//!   repeat with that new mast as the next guess until it stops moving (at most
//!   8 iterations). One conversion is not enough: it measures `width_axis` with
//!   the target tier still at the bootstrap mast.
//! - [`TierTarget::GirdleThicknessMm`]/[`TierTarget::TableWidthMm`] resolve by
//!   bracketing the target tier's own mast around its CURRENTLY authored value
//!   (never the bootstrap's `0.0`, which is measurably degenerate for a girdle/
//!   table facet -- see [`bisect_tier_mast`]'s own doc comment), expanding
//!   geometrically until the resulting solid's measured girdle thickness/table
//!   width straddles the target, then bisecting to a tolerance of `1e-4` of the
//!   girdle diameter. [`TargetResolveError::CannotBracket`] when the search
//!   exhausts its budget without finding two measurable points that straddle the
//!   target (e.g. a target wider than the preform itself allows).
//!
//! # The girdle thickness a target aims at
//!
//! [`TierTarget::GirdleThicknessMm`] measures the girdle at its LINE, the figure a cutter
//! measures: the thinnest thickness at the line corners
//! of [`Design::girdle_level`] (where girdle faces meet, and the ends of level edges), from
//! [`super::girdle_level::GirdleLevel::thickness_range`]. Not the band's full height
//! (`StoneProportions::girdle_thickness`), which on a faceted brilliant includes the V-scallops
//! the break facets cut between the girdle corners. A design without a girdle line (no
//! `|angle| = 90` tier, or no crown or pavilion facet meeting its girdle faces at a line corner)
//! falls back to the band.
//!
//! The search stays a bracket-and-bisect: the line thickness is continuous and piecewise linear
//! in the mast, as the band is (each line corner moves linearly between topology changes). A
//! corner that joins or leaves the line as the tier moves (a level edge appearing where a facet
//! at the face's azimuth starts to reach it) does so at the height the edge already had there,
//! so the thinnest figure does not jump; where a design does jump (the line disappears and the
//! band takes over), the bisection still ends, at the jump, after its fixed budget.
//!
//! A design saved with a girdle-thickness target re-solves to the line thickness when it is
//! opened.
//!
//! # Legacy entry points resolve targets too
//!
//! [`super::Design::solve`]/[`super::Design::resolve_dirty`] call through
//! [`super::Design::solve_with`]/[`super::Design::resolve_dirty_with`]
//! internally, so they benefit from target resolution exactly like those `_with`
//! entry points. Both wrappers' `Err` type is
//! [`super::DesignSolveError`] (not the narrower [`super::MissingAnchor`] their
//! signatures used before this module existed) precisely so a resolution failure
//! -- missing girdle diameter, or a target that cannot be bracketed -- has
//! somewhere real to go instead of panicking; see [`TargetResolveError`]'s own
//! [`std::fmt::Display`] impl for the status-strip sentence each variant renders
//! as.

use super::{Design, DesignSolveError, TierId};
use crate::yield_metrics::mm_per_unit;
use pleochroma::geometry::{
    meet_solver::{MeetConstraint, MeetTierInput, SolveControl, SolvedTier},
    stone_metrics::{
        SolidMetrics, SolidStatus, StoneProportions, build_solid_mesh_and_measure, measure_solid,
    },
};
use std::collections::BTreeMap;

/// One authoring-level target a cutter states in real units, resolved to a
/// [`MeetConstraint::ScaleReference`] before solving -- see the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TierTarget {
    /// "Cut this facet to depth `_` mm" -- the tier's own mast, converted via
    /// [`crate::yield_metrics::mm_per_unit`].
    DepthMm(f64),
    /// "This design's girdle should be `_` mm thick" -- bisects the target
    /// tier's own mast until the girdle's thinnest thickness at its girdle LINE
    /// ([`Design::girdle_level`], see the module docs; converted to mm) matches,
    /// or the band's [`StoneProportions::girdle_thickness`] for a design without
    /// a girdle line. Ordinarily authored on the tier
    /// `pleochroma::geometry::meet_solver::classify_blocks` would call the
    /// girdle (angle at or near 90 degrees), but this resolves the same way
    /// regardless of which tier carries it.
    GirdleThicknessMm(f64),
    /// "This design's table should be `_` mm wide" -- bisects the target
    /// tier's own mast until the table facet's own measured width (derived
    /// from [`StoneProportions::table_percent`] and the solid's own
    /// `width_axis`, converted to mm) matches.
    TableWidthMm(f64),
}

/// Every way [`Design::resolved_meet_tier_inputs`] can fail to turn
/// [`Design::tier_targets`] into real masts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetResolveError {
    /// A [`TierTarget`] is set but [`Design::girdle_diameter_mm`] is not -- there
    /// is no mm-per-unit scale to convert a millimetre target through. Surfaced
    /// as the status-strip problem text.
    MissingGirdleDiameter,
    /// [`TierTarget::GirdleThicknessMm`]/[`TierTarget::TableWidthMm`]'s bisection
    /// could not bracket the target within its search bound, or the design
    /// stopped measuring as a closed solid partway through the search --
    /// nothing this crate should loop forever chasing.
    CannotBracket { tier_index: usize },
    /// [`TierTarget::DepthMm`]'s bootstrap pass solved, but the design did not
    /// measure as a real closed solid (or measured a non-positive width), so
    /// there is no `width_axis` to derive a scale from.
    CannotMeasure { tier_index: usize },
    /// The bootstrap or bisection solve itself failed (a missing anchor
    /// elsewhere in the design, cancellation, or the solver's own plane cap).
    Anchor(Box<DesignSolveError>),
}

impl std::fmt::Display for TargetResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingGirdleDiameter => write!(
                f,
                "cut-to-depth needs a girdle diameter -- set one in Design settings"
            ),
            Self::CannotBracket { tier_index } => write!(
                f,
                "tier {}'s target could not be bracketed -- widen or remove it",
                tier_index + 1
            ),
            Self::CannotMeasure { tier_index } => write!(
                f,
                "tier {}'s target could not be resolved: the design does not currently measure \
                 as a closed solid",
                tier_index + 1
            ),
            Self::Anchor(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TargetResolveError {}

impl From<DesignSolveError> for TargetResolveError {
    fn from(e: DesignSolveError) -> Self {
        Self::Anchor(Box::new(e))
    }
}

/// Which measured figure [`bisect_tier_mast`]'s generic bracket search chases: a mast-unit
/// length of a solved-and-measured candidate, converted to mm by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Figure {
    /// The girdle's thinnest thickness at its girdle line, or the band without a line (see the
    /// module docs, "The girdle thickness a target aims at").
    GirdleThickness,
    /// The table facet's own width.
    TableWidth,
}

/// `figure` of `design` solved as `solved`, whose stone measures `proportions`/`metrics`, in
/// design units.
fn figure_mast(
    figure: Figure,
    design: &Design,
    solved: &[SolvedTier],
    proportions: &StoneProportions,
    metrics: &SolidMetrics,
) -> Option<f64> {
    match figure {
        Figure::GirdleThickness => design
            .girdle_level(solved)
            .map_or(proportions.girdle_thickness, |level| {
                Some(level.thickness_range().0.max(0.0))
            }),
        Figure::TableWidth => proportions
            .table_percent
            .map(|pct| pct / 100.0 * metrics.width_axis),
    }
}

/// Solves `design` with tier `tier_index` pinned to `mast`, and returns
/// `figure` converted to millimetres via `girdle_mm`/the resulting
/// `width_axis` -- `None` for anything that stops this from being measurable
/// (solve failure, non-closed solid, non-positive width).
fn measured_value_mm(
    design: &Design,
    tier_index: usize,
    mast: f64,
    girdle_mm: f64,
    figure: Figure,
) -> Option<f64> {
    let mut trial = design.clone();
    trial.tiers[tier_index].constraint = MeetConstraint::ScaleReference(mast);
    let solved = trial.solve_with(&SolveControl::default()).ok()?;
    let planes = trial.planes_from_solved(&solved);
    let (SolidStatus::Closed(mesh), Some(metrics)) = build_solid_mesh_and_measure(&planes) else {
        return None;
    };
    if metrics.width_axis <= 1e-9 {
        return None;
    }
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    let value_mast = figure_mast(figure, &trial, &solved, &proportions, &metrics)?;
    let scale = mm_per_unit(girdle_mm, metrics.width_axis)?;
    Some(value_mast * scale)
}

/// `constraint`'s own mast when it is a [`MeetConstraint::ScaleReference`], else
/// `1.0` -- [`bisect_tier_mast`]'s fallback seed for a target-bearing tier that
/// has never carried a real scale value (e.g. a brand-new tier the editor just
/// added).
const fn constraint_mast(constraint: &MeetConstraint) -> f64 {
    match constraint {
        MeetConstraint::ScaleReference(m) => *m,
        MeetConstraint::MeetExisting | MeetConstraint::MeetNamed(_) => 1.0,
    }
}

/// Brackets tier `tier_index`'s own mast around `seed_mast`, its authored value from
/// before this resolution pass (not the bootstrap's `0.0`, see
/// [`Design::resolved_meet_tier_inputs`]). A girdle or table facet at mast `0.0` sits on
/// the stone's axis and does not measure as a closed solid, so no target could be
/// bracketed from there.
///
/// # Direction is measured, never assumed
///
/// A bigger mast does not always mean a bigger measured figure: on the standard round
/// brilliant, girdle thickness and table width both shrink as their own tier's mast
/// grows. The expansion direction therefore comes from a small nudge off `seed_mast`
/// (does the figure increase with mast?), combined with which side of `target_mm` the
/// seed's own figure sits on.
///
/// # Algorithm
///
/// Expands from `seed_mast` in the measured direction (doubling each step)
/// until the measured figure's sign relative to `target_mm` flips (a real
/// bracket, found by the SIGN of `measured - target_mm`, not by magnitude --
/// correct regardless of which way the relationship runs), then bisects
/// between the two bracket ends to a tolerance of `1e-4 * girdle_mm` -- at
/// most 20 expansion steps plus 24 bisection steps. A trial mast that fails to measure
/// (solve failure, non-closed solid, or a solid without this figure) is walked back halfway
/// toward the last point that did measure. Only exhausting the search budget reports
/// [`TargetResolveError::CannotBracket`].
fn bisect_tier_mast(
    design: &Design,
    tier_index: usize,
    girdle_mm: f64,
    target_mm: f64,
    figure: Figure,
    seed_mast: f64,
) -> Result<f64, TargetResolveError> {
    let bracket_err = || TargetResolveError::CannotBracket { tier_index };
    let measure = |mast: f64| measured_value_mm(design, tier_index, mast, girdle_mm, figure);
    let tolerance = 1e-4 * girdle_mm.abs().max(1e-6);

    let seed = if seed_mast.is_finite() && seed_mast.abs() > 1e-9 {
        seed_mast.abs()
    } else {
        1.0
    };

    // An anchor point known to measure, walked toward `seed` (halving) when
    // `seed` itself does not -- e.g. a tier another target's own bootstrap pass
    // left pinned at `0.0`.
    let mut anchor = seed;
    let mut anchor_val = measure(anchor);
    let mut shrink_tries = 0;
    while anchor_val.is_none() && shrink_tries < 16 {
        anchor *= 0.5;
        anchor_val = measure(anchor);
        shrink_tries += 1;
    }
    let anchor_val = anchor_val.ok_or_else(bracket_err)?;
    let anchor_diff = anchor_val - target_mm;
    if anchor_diff.abs() <= tolerance {
        return Ok(anchor);
    }

    // Which mast direction moves the measured figure toward `target_mm` --
    // measured with a small (0.1%) nudge rather than assumed, and small enough
    // not to overshoot a narrow measurable window itself (see "Direction is
    // measured" above). `increases`: `true` if the figure grows as mast grows
    // near `anchor`. `None` (the nudge itself did not measure) tries the
    // opposite tiny nudge before giving up and defaulting to growing the mast.
    let epsilon = (anchor * 1e-3).max(1e-9);
    let increases = measure(anchor + epsilon).map_or_else(
        || measure((anchor - epsilon).max(1e-9)).is_none_or(|v| v < anchor_val),
        |v| v > anchor_val,
    );
    // We need the figure to grow (`anchor_diff < 0.0`) or shrink
    // (`anchor_diff > 0.0`) to reach `target_mm`; combined with whether it
    // grows or shrinks as mast grows, that tells us which way to move mast.
    let grow_mast = increases == (anchor_diff < 0.0);

    // Expand in that direction until the SIGN of `measured - target_mm` flips.
    let mut probe = anchor;
    let mut last_good = anchor;
    let mut bound: Option<(f64, f64)> = None;
    for _ in 0..20 {
        let mut next = if grow_mast { probe * 2.0 } else { probe * 0.5 };
        let mut value = measure(next);
        let mut retries = 0;
        while value.is_none() && retries < 6 {
            next = f64::midpoint(next, last_good);
            value = measure(next);
            retries += 1;
        }
        let Some(v) = value else {
            return Err(bracket_err());
        };
        last_good = next;
        probe = next;
        let diff = v - target_mm;
        if diff.abs() <= tolerance {
            return Ok(next);
        }
        if diff.signum() != anchor_diff.signum() {
            bound = Some((next, diff));
            break;
        }
    }
    let Some((bound_mast, bound_diff)) = bound else {
        return Err(bracket_err());
    };

    // `below`/`above` name which SIDE of `target_mm` each bracket end sits on
    // (not which mast is numerically bigger -- see "Direction is measured"
    // above for why the two can point either way).
    let (mut below, mut above) = if anchor_diff < 0.0 {
        (anchor, bound_mast)
    } else {
        (bound_mast, anchor)
    };
    debug_assert!(anchor_diff.signum() != bound_diff.signum() || bound_diff == 0.0);
    for _ in 0..24 {
        let mid = f64::midpoint(below, above);
        let mid_val = measure(mid).ok_or_else(bracket_err)?;
        let diff = mid_val - target_mm;
        if diff.abs() <= tolerance {
            return Ok(mid);
        }
        if diff < 0.0 {
            below = mid;
        } else {
            above = mid;
        }
    }
    Ok(f64::midpoint(below, above))
}

impl Design {
    /// The [`TierTarget`] authored for the tier CURRENTLY at `index`, if any --
    /// looked up via [`Self::tier_ids`], the same positional-to-stable-id
    /// translation [`Self::cheater_offset_deg`]/[`Self::tier_note`] use for their
    /// own maps.
    #[must_use]
    pub fn tier_target(&self, index: usize) -> Option<TierTarget> {
        self.tier_ids
            .get(index)
            .and_then(|id| self.tier_targets.get(id))
            .copied()
    }

    /// The [`TierTarget`] for a specific [`TierId`], regardless of that tier's
    /// current position -- for a caller (e.g. [`Self::resolved_meet_tier_inputs`])
    /// iterating `tier_ids` directly.
    #[must_use]
    pub fn tier_target_for_id(&self, id: TierId) -> Option<TierTarget> {
        self.tier_targets.get(&id).copied()
    }

    /// [`Self::meet_tier_inputs`], but with every tier that carries a
    /// [`TierTarget`] resolved to a real [`MeetConstraint::ScaleReference`] first
    /// -- see the module docs for the resolution order (depth targets, then
    /// bisected girdle-thickness/table-width targets) and why this is a no-op,
    /// `self.meet_tier_inputs()` returned unchanged, whenever
    /// [`Self::tier_targets`] is empty.
    ///
    /// # Errors
    ///
    /// [`TargetResolveError`] -- see that type's own variants.
    pub fn resolved_meet_tier_inputs(&self) -> Result<Vec<MeetTierInput>, TargetResolveError> {
        if self.tier_targets.is_empty() {
            return Ok(self.meet_tier_inputs());
        }
        let girdle_mm = self
            .girdle_diameter_mm
            .ok_or(TargetResolveError::MissingGirdleDiameter)?;

        // Bootstrap: every target-bearing tier temporarily pinned at mast 0.0,
        // solved once so a `DepthMm` target has a real `width_axis` to convert
        // through. `tier_targets` is cleared on the clone FIRST -- critical, not
        // cosmetic: `Design::clone` carries `tier_targets` over verbatim, and
        // `Design::solve_with` calls back into this very method, so a clone that
        // still had entries would recurse into another bootstrap pass forever
        // instead of taking the (now-correct) empty-map fast path.
        let mut bootstrap = self.clone();
        bootstrap.tier_targets.clear();
        for index in 0..bootstrap.tiers.len() {
            if self.tier_target(index).is_some() {
                bootstrap.tiers[index].constraint = MeetConstraint::ScaleReference(0.0);
            }
        }
        let bootstrap_solved = bootstrap.solve_with(&SolveControl::default())?;
        let bootstrap_metrics = measure_solid(&bootstrap.planes_from_solved(&bootstrap_solved));

        let mut resolved = bootstrap;
        for index in 0..resolved.tiers.len() {
            if let Some(TierTarget::DepthMm(target_mm)) = self.tier_target(index) {
                // Fixed-point iteration: the bootstrap solve measured `width_axis` with this
                // tier at mast `0.0`, so one conversion inherits that placement. Each further
                // pass re-measures with the tier at its latest guess, until the guess stops
                // moving (at most 8 passes; in practice 2-3).
                let mut width_axis = bootstrap_metrics
                    .as_ref()
                    .map(|m| m.width_axis)
                    .filter(|w| *w > 1e-9)
                    .ok_or(TargetResolveError::CannotMeasure { tier_index: index })?;
                let mut mast = 0.0_f64;
                for _ in 0..8 {
                    let scale = mm_per_unit(girdle_mm, width_axis)
                        .ok_or(TargetResolveError::CannotMeasure { tier_index: index })?;
                    let next_mast = target_mm / scale;
                    let converged = (next_mast - mast).abs() <= 1e-9 * next_mast.abs().max(1.0);
                    mast = next_mast;
                    if converged {
                        break;
                    }
                    let mut trial = resolved.clone();
                    trial.tier_targets.clear();
                    trial.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
                    let trial_solved = trial.solve_with(&SolveControl::default())?;
                    width_axis = measure_solid(&trial.planes_from_solved(&trial_solved))
                        .map(|m| m.width_axis)
                        .filter(|w| *w > 1e-9)
                        .ok_or(TargetResolveError::CannotMeasure { tier_index: index })?;
                }
                resolved.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
            }
        }

        for index in 0..resolved.tiers.len() {
            let seed_mast = constraint_mast(&self.tiers[index].constraint);
            let mast = match self.tier_target(index) {
                Some(TierTarget::GirdleThicknessMm(target_mm)) => Some(bisect_tier_mast(
                    &resolved,
                    index,
                    girdle_mm,
                    target_mm,
                    Figure::GirdleThickness,
                    seed_mast,
                )?),
                Some(TierTarget::TableWidthMm(target_mm)) => Some(bisect_tier_mast(
                    &resolved,
                    index,
                    girdle_mm,
                    target_mm,
                    Figure::TableWidth,
                    seed_mast,
                )?),
                _ => None,
            };
            if let Some(mast) = mast {
                resolved.tiers[index].constraint = MeetConstraint::ScaleReference(mast);
            }
        }

        Ok(resolved.meet_tier_inputs())
    }
}

/// Every tier's [`TierTarget`], keyed by [`TierId`]; see the module docs.
/// `pub` rather than `pub(crate)`: this module is private, so the two are equivalent,
/// and clippy's `redundant_pub_crate` prefers `pub`.
pub type TierTargetMap = BTreeMap<TierId, TierTarget>;
