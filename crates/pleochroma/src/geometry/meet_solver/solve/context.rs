//! [`SolveContext`]: precomputed, immutable per-design solve state shared by
//! every pipeline run.

use glam::DVec3;

use super::super::{
    BLANK_DOMINATION_FACTOR, DEFAULT_PLAUSIBLE_SCALE, MAX_PLANES, MeetConstraint, MeetTierInput,
    SolveStrategy, SolvedTier,
    blocks::{Block, classify_blocks, tier_sides},
    candidates::tier_normals,
    names::MeetNameResolver,
};

/// Precomputed, immutable per-design solve state: everything every pipeline run
/// shares (normals, block classification, name resolution, anchors, priors).
///
/// Built once per design; [`Self::run_pipeline`](super::pipeline) can then run
/// any number of times -- with different decision overrides -- without
/// re-deriving any of it. That reuse is what makes
/// [`solve_meet_points_verified`](super::super::solve_meet_points_verified)'s
/// externally-scored repair search affordable.
pub(in crate::geometry::meet_solver) struct SolveContext<'a> {
    pub(in crate::geometry::meet_solver) tiers: &'a [MeetTierInput],
    pub(super) normals: Vec<Vec<DVec3>>,
    pub(super) blocks: Vec<Block>,
    pub(super) resolved_named: Vec<Vec<usize>>,
    pub(in crate::geometry::meet_solver) is_anchor: Vec<bool>,
    pub(super) scale_prior: f64,
    /// The design's absolute scale, rounded to the nearest power of two
    /// (`2^round(log2(scale_prior))`). The geometric thresholds of this module
    /// ([`super::super::EPS_FEAS`], [`super::super::LEVEL_TOL`] and others) are absolute
    /// and tuned for masts of order 1. `run_pipeline` therefore divides every anchor mast
    /// by this value, and `to_solved` multiplies the results back. A scale that rounds
    /// to `1.0` makes both steps exact no-ops.
    pub(super) scale_norm: f64,
    pub(super) domination_limit: f64,
    pub(in crate::geometry::meet_solver) total_planes: usize,
}

/// Rounds a strictly positive, finite scale to the nearest power of two. Other input
/// falls back to `1.0` (no normalisation). Callers already pass a clamped, finite scale,
/// so this only guards the division.
#[expect(
    clippy::cast_possible_truncation,
    reason = "scale is finite and positive, so log2 rounds to an exponent within about +-1075"
)]
fn pow2_scale_norm(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        2f64.powi(scale.log2().round() as i32)
    } else {
        1.0
    }
}

impl<'a> SolveContext<'a> {
    /// Builds a new context, precomputing per-tier instance normals, block
    /// classification, resolved `"Meet <names>"` references, anchor flags, and
    /// the scale prior/domination limit -- see the type doc for why this is
    /// done once and reused.
    pub(in crate::geometry::meet_solver) fn new(
        gear_teeth_abs: u32,
        tiers: &'a [MeetTierInput],
    ) -> Self {
        let sides = tier_sides(tiers);
        let normals: Vec<Vec<DVec3>> = tiers
            .iter()
            .zip(&sides)
            .map(|(t, &crown)| tier_normals(gear_teeth_abs, t.angle_deg, &t.indices, crown))
            .collect();

        let blocks = classify_blocks(tiers);

        // Resolve every MeetNamed tier's references up front via the shared
        // [`MeetNameResolver`] rule set (see its doc comment).
        let resolver = MeetNameResolver::new(tiers);
        let resolved_named: Vec<Vec<usize>> = tiers
            .iter()
            .enumerate()
            .map(|(i, t)| match &t.constraint {
                MeetConstraint::MeetNamed(names) => {
                    let mut refs = resolver.resolve_names(names).refs;
                    refs.retain(|&r| r != i);
                    refs
                }
                _ => Vec::new(),
            })
            .collect();

        let is_anchor: Vec<bool> = tiers
            .iter()
            .map(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_)))
            .collect();
        // A plain `.fold(0.0, f64::max)` would drop a NaN anchor, since `max` returns
        // the other argument. Propagate the NaN so it stays visible in `scale_prior`.
        // Callers already reject non-finite input (`first_non_finite_tier`).
        let scale_prior = tiers
            .iter()
            .filter_map(|t| match &t.constraint {
                MeetConstraint::ScaleReference(v) => Some(v.abs()),
                _ => None,
            })
            .fold(0.0_f64, |acc, v| {
                if acc.is_nan() || v.is_nan() {
                    f64::NAN
                } else {
                    acc.max(v)
                }
            });
        let scale_prior = if scale_prior > 1e-9 {
            scale_prior
        } else {
            DEFAULT_PLAUSIBLE_SCALE
        };
        let scale_norm = pow2_scale_norm(scale_prior);
        let domination_limit = BLANK_DOMINATION_FACTOR * (scale_prior / scale_norm);

        let total_planes = 6 + normals.iter().map(Vec::len).sum::<usize>();
        Self {
            tiers,
            normals,
            blocks,
            resolved_named,
            is_anchor,
            scale_prior,
            scale_norm,
            domination_limit,
            total_planes,
        }
    }

    /// The over-[`MAX_PLANES`] early-out result: anchors keep their given masts,
    /// everything else is a flagged placeholder.
    pub(in crate::geometry::meet_solver) fn failed_solved(&self) -> Vec<SolvedTier> {
        let total_planes = self.total_planes;
        self.tiers
            .iter()
            .map(|t| match &t.constraint {
                MeetConstraint::ScaleReference(v) => SolvedTier {
                    mast: v.abs(),
                    strategy: SolveStrategy::ScaleReference,
                    detail: "given (scale reference)".to_string(),
                },
                _ => SolvedTier {
                    mast: self.scale_prior,
                    strategy: SolveStrategy::Failed,
                    detail: format!(
                        "design has {total_planes} planes, above the {MAX_PLANES}-plane cap \
                         for candidate-vertex enumeration"
                    ),
                },
            })
            .collect()
    }
}
