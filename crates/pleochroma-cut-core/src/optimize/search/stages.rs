//! The first two stages of [`optimize_design_with`](super::optimize_design_with).
//!
//! They are measuring
//! the starting point ([`baseline_report`]), the coordinate-descent stage ([`run_search`]) and
//! the Nelder-Mead hand-off ([`run_polish_stage`]). Proposals go through the meet-preserving
//! projection when the request carries one.

use super::{
    Baseline, CoordinateStageOutcome, OptimizeConfig, PolishStageOutcome, SearchHooks, SearchStage,
    seeded_permutation,
};
use crate::{
    design::{Design, DesignSolveError},
    manufacturability::{DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability},
    optimize::{
        candidate::{
            BaselineWarningCounts, CandidateOutcome, ScoredStone, SearchContext,
            build_free_angle_candidate, evaluate_candidate_pair, evaluate_in_context,
            free_tier_indices_with, measure_with_tone, measure_with_tone_cancellable,
            shape_penalty_for_planes, yield_loss_pct,
        },
        guard::ShapeGuard,
        objective::ObjectiveFidelity,
        options::OptimizeOptions,
        polish,
        pool::CandidatePool,
        space::measure_anchors,
    },
};
use pleochroma::{
    geometry::stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    optics::materials::GemMaterial,
};

#[cfg(doc)]
use super::optimize_design;

/// What one coordinate descent owns when several share a config: see [`run_search`].
pub(super) struct StartRun {
    /// The evaluation budget of this descent (a soft cap, checked per tier decision).
    pub(super) max_evaluations: usize,
    /// The seed of this descent's sweep orders.
    pub(super) seed: u64,
    /// The `Fast` score of the design the descent starts from.
    pub(super) initial_score: f32,
}

/// Builds [`optimize_design`]'s [`Baseline`].
///
/// Reports [`SearchStage::BaselineFull`] (evaluations `0`, since none of `config`'s budget
/// is spent here) through `hooks` before the one [`ObjectiveFidelity::Full`] scoring this
/// function performs. That scoring is the expensive part (see the parent module's
/// "Cost" section), and without the report a progress display would sit on a frozen
/// counter before the search had started.
///
/// # Errors
///
/// Propagates [`Design::solve`]'s [`DesignSolveError`]. A `Full` scoring stopped by
/// [`SearchHooks::cancel`] is reported as the solve's own cancelled error
/// (`DesignSolveError::Solve(SolveError::Cancelled)`), the same cancellation the callers
/// already read from the flag.
pub(super) fn baseline_report(
    design: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    options: &OptimizeOptions,
    hooks: &SearchHooks<'_>,
) -> Result<Baseline, DesignSolveError> {
    let preservation = match &options.preservation_baseline {
        Some(baseline) => baseline.clone(),
        None => std::sync::Arc::new(
            crate::meet_preservation::MeetPreservationBaseline::capture_with(design, hooks.cancel)
                .map_err(DesignSolveError::Preservation)?,
        ),
    };
    // The validating solve is the baseline's only solve beyond the capture's own: its tiers
    // are those of `design` as passed.
    let (_, baseline_solved) = preservation
        .validate_and_solve_with(design, options.preservation_policy, hooks.cancel)
        .map_err(DesignSolveError::Preservation)?;
    let weights = &config.weights;
    let lighting = config.lighting;
    hooks.report(0, SearchStage::BaselineFull);
    let baseline_planes = design.planes_from_solved(&baseline_solved);
    let warnings = BaselineWarningCounts::count(&check_manufacturability(
        design,
        &baseline_solved,
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    ));
    // The stone with its concave tools (none for a flat design): every score of the run,
    // the baseline's included, measures the stone the cutter holds.
    let baseline_stone = ScoredStone::of(design, &baseline_solved, &baseline_planes);
    let full_timer = preservation.telemetry.phase(5);
    // The `Full` sweep stops when the caller cancels; a stopped sweep is a cancelled baseline
    // (the error the solve reports for it), never a scored one.
    let Some((before, tone_before, before_tone_loss)) = measure_with_tone_cancellable(
        baseline_stone.finished(),
        material,
        weights,
        ObjectiveFidelity::Full,
        lighting,
        hooks.cancel,
    ) else {
        return Err(DesignSolveError::Solve(
            pleochroma::geometry::meet_solver::SolveError::Cancelled,
        ));
    };
    drop(full_timer);
    let baseline_metrics = measure_solid(&baseline_planes);
    let before_yield_loss_pct = yield_loss_pct(
        design,
        baseline_stone
            .yield_metrics(&baseline_planes, baseline_metrics.as_ref())
            .as_ref(),
    );
    // The starting design pays the same shape penalty its candidates do (zero when the
    // target was read off it), so every score in the run stays comparable.
    let before_penalty = shape_penalty_for_planes(
        options.active_shape_target(),
        baseline_metrics.as_ref(),
        &baseline_planes,
    );
    let before_score =
        weights.score_with_tone(&before, before_yield_loss_pct, before_tone_loss) + before_penalty;
    // Same planes and yield, re-scored at `Fast` so `run_search` compares like with like
    // (see `Baseline::before_score_fast`). The tone is measured only when it is weighted.
    let fast_timer = preservation.telemetry.phase(4);
    let (before_fast, _, before_fast_tone_loss) = measure_with_tone(
        baseline_stone.finished(),
        material,
        weights,
        ObjectiveFidelity::Fast,
        lighting,
    );
    drop(fast_timer);
    let before_score_fast =
        weights.score_with_tone(&before_fast, before_yield_loss_pct, before_fast_tone_loss)
            + before_penalty;
    let mut free = free_tier_indices_with(design, options);
    let anchors = measure_anchors(design, options, &free, &baseline_solved, &baseline_planes);
    // An anchored tier with no facet plane to read an azimuth from cannot turn about its
    // hinge, so it is not part of this run.
    free.retain(|&tier| {
        options.anchored_hinge(design, tier).is_none() || anchors.contains_key(&tier)
    });
    let guard = options.min_girdle_fraction.and_then(|fraction| {
        // Nothing closes without a closed start, so nothing needs guarding.
        let SolidStatus::Closed(mesh) = build_solid_mesh(&baseline_planes) else {
            return None;
        };
        Some(
            ShapeGuard::measure(design, &baseline_solved, &baseline_planes, &mesh, fraction)
                .with_thinnest_fraction(options.min_girdle_thinnest_fraction),
        )
    });
    Ok(Baseline {
        preservation,
        before,
        before_score,
        before_score_fast,
        before_yield_loss_pct,
        tone_before,
        warnings,
        free,
        anchors,
        guard,
    })
}

/// [`optimize_design`]'s coordinate-descent stage itself -- see that function's own
/// doc comment for the algorithm. Returns the design the stage ended on, that
/// design's own [`ObjectiveFidelity::Fast`] score (so [`run_polish_stage`] does not
/// have to re-evaluate the point it starts from), how many candidate evaluations
/// were spent, and whether [`SearchHooks::cancel`] cut it short.
///
/// When [`OptimizeConfig::polish_start_step_deg`] is disabled (`None` or non-positive),
/// the hand-off check below never fires, so the candidates tried and accepted are the
/// same as the plain coordinate search.
///
/// `pool` collects every accepted candidate that beats the starting design's own
/// `Fast` score (see [`CandidatePool`]): the candidates the request keeps plus the
/// full-fidelity gate's reserve of fallbacks. The offers cost a clone of the accepted
/// design each; a disabled pool ignores them.
///
/// `start` carries the three values a run owns when several starts share one config:
/// its slice of the evaluation budget, its sweep seed, and the `Fast` score of `design`
/// (the point it starts from). A single-start search passes the config's own values and
/// the baseline's `Fast` score, so nothing changes for it.
pub(super) fn run_search(
    design: &Design,
    config: &OptimizeConfig,
    ctx: &SearchContext,
    baseline: &Baseline,
    mut pool: CandidatePool,
    hooks: &SearchHooks<'_>,
    start: &StartRun,
) -> CoordinateStageOutcome {
    let free = &baseline.free;
    let mut current = design.clone();
    // Seeded from the `Fast` baseline, never `baseline.before_score` (`Full`): every
    // candidate compared against `current_score` is scored at `Fast`, so a `Full` start
    // could accept a candidate that only reproduces the starting point (see
    // `Baseline::before_score_fast`).
    let mut current_score = start.initial_score;
    let mut evaluations = 0usize;
    let mut step_deg = config.initial_step_deg;
    let mut sweep_index = 0u64;
    let mut cancelled = false;

    'search: while step_deg >= config.min_step_deg && evaluations < start.max_evaluations {
        let order = seeded_permutation(
            free.len(),
            start.seed ^ sweep_index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        sweep_index += 1;
        let mut improved_this_sweep = false;

        // The axial family is tried first, within the same evaluation budget: a multi-way
        // meet need not wait for one coordinate to cross an infeasible region.
        for sign in [1.0_f64, -1.0] {
            if hooks.is_cancelled() {
                cancelled = true;
                break 'search;
            }
            if evaluations >= start.max_evaluations {
                break;
            }
            let factor = (sign * step_deg.to_radians()).exp();
            evaluations += 1;
            if let Some(preference) = crate::optimize::families::axial_preference(&current, factor)
                && let CandidateOutcome::Accepted { design, score } =
                    evaluate_in_context(&preference, ctx)
            {
                if score < baseline.before_score_fast {
                    pool.offer_design((*design).clone(), free, score);
                }
                if score < current_score {
                    current = *design;
                    current_score = score;
                    improved_this_sweep = true;
                }
            }
            hooks.report(evaluations, SearchStage::Coordinate);
        }

        for &free_slot in &order {
            if hooks.is_cancelled() {
                cancelled = true;
                break 'search;
            }
            if evaluations >= start.max_evaluations {
                break;
            }
            let tier_index = free[free_slot];
            let original_deg = current.tiers[tier_index].angle_deg;

            let pair = evaluate_candidate_pair(
                &current,
                tier_index,
                original_deg,
                step_deg,
                ctx,
                current_score,
            );
            evaluations += pair.evaluations;
            hooks.report(evaluations, SearchStage::Coordinate);

            for (candidate, score) in pair.accepted {
                if score < baseline.before_score_fast {
                    pool.offer_design(candidate, free, score);
                }
            }
            if let Some((new_design, new_score)) = pair.best {
                current = new_design;
                current_score = new_score;
                improved_this_sweep = true;
            }
        }

        if !improved_this_sweep {
            step_deg /= 2.0;
            // Once the step is this fine, further axis-aligned halving only grinds on a
            // ridge, so hand off to the polish stage (see `optimize_design`'s "Two stages").
            let should_hand_off = config
                .polish_start_step_deg
                .is_some_and(|threshold| threshold > 0.0 && step_deg < threshold);
            if should_hand_off {
                break;
            }
        }
    }

    CoordinateStageOutcome {
        design: current,
        score: current_score,
        evaluations,
        cancelled,
        pool,
    }
}

/// The polish stage: runs [`polish::run_polish`] from the coordinate stage's end point and
/// applies its result to `current`'s free tiers if it strictly improves on `current_score`.
/// The scoring closure goes through [`build_free_angle_candidate`] and [`evaluate_in_context`],
/// the same solve, closure, guard and manufacturability gate as the coordinate stage. Only
/// free tiers (`baseline.free`) are ever written. Every finite point scored below the
/// starting design's `Fast` score is also offered to the candidate pool.
///
/// Disabled ([`OptimizeConfig::polish_start_step_deg`] is `None` or non-positive) it returns
/// at once with zero evaluations.
///
/// Each evaluation reports [`SearchStage::Polish`] with a running total that starts at
/// `coord.evaluations`, so a caller's counter keeps climbing across the hand-off. Only
/// called with `coord.cancelled == false`.
pub(super) fn run_polish_stage(
    _design: &Design,
    baseline: &Baseline,
    config: &OptimizeConfig,
    ctx: &SearchContext,
    hooks: &SearchHooks<'_>,
    coord: CoordinateStageOutcome,
) -> PolishStageOutcome {
    let free = &baseline.free;
    let CoordinateStageOutcome {
        design: current,
        score: current_score,
        evaluations: coord_evaluations,
        cancelled: _,
        mut pool,
    } = coord;
    let Some(start_step) = config
        .polish_start_step_deg
        .filter(|&step| step > 0.0 && !free.is_empty())
    else {
        return PolishStageOutcome {
            current,
            evaluations: 0,
            improvement: 0.0,
            cancelled: false,
            pool,
        };
    };

    let starting_point: Vec<f64> = free.iter().map(|&i| current.tiers[i].angle_deg).collect();
    let max_evaluations = config
        .polish_max_evaluations
        .unwrap_or_else(|| 3 * free.len() + 20);
    let min_spread = config.min_step_deg / 2.0;

    let mut polish_evaluations_done = 0usize;
    let mut best_polished: Option<(Design, f32)> = None;
    let evaluate = |angles: &[f64]| -> f32 {
        let score = build_free_angle_candidate(&current, free, &starting_point, angles, ctx.space)
            .map_or(f32::INFINITY, |candidate| {
                match evaluate_in_context(&candidate, ctx) {
                    CandidateOutcome::Rejected => f32::INFINITY,
                    CandidateOutcome::Accepted { score, design } => {
                        if score < baseline.before_score_fast {
                            pool.offer_design((*design).clone(), free, score);
                        }
                        if score < current_score
                            && best_polished.as_ref().is_none_or(|(_, best)| score < *best)
                        {
                            best_polished = Some((*design, score));
                        }
                        score
                    }
                }
            });
        polish_evaluations_done += 1;
        hooks.report(
            coord_evaluations + polish_evaluations_done,
            SearchStage::Polish,
        );
        score
    };

    let result = polish::run_polish(
        &starting_point,
        current_score,
        start_step,
        max_evaluations,
        min_spread,
        &|| hooks.is_cancelled(),
        evaluate,
    );

    debug_assert_eq!(result.point.len(), free.len());
    debug_assert!(result.score <= current_score || !current_score.is_finite());
    let (current, improvement) = best_polished.map_or((current, 0.0), |(improved, score)| {
        (improved, current_score - score)
    });
    PolishStageOutcome {
        current,
        evaluations: result.evaluations,
        improvement,
        cancelled: result.cancelled,
        pool,
    }
}

#[cfg(test)]
#[path = "stages_tests.rs"]
mod tests;
