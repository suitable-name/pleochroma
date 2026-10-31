//! The last step of [`optimize_design_with`](super::optimize_design_with): measuring the
//! ending point(s) at full fidelity and turning them into ranked candidates
//! ([`build_result`]). Split from the parent module so the file stays readable; the
//! functions were moved here, not rewritten. Every scored state is re-validated against the
//! captured meet network first (`score_state`), and `provenance::stamp_result` re-checks it
//! when it stamps the candidates.

use super::{AngleChange, Baseline, OptimizeOutcome, SearchHooks, SearchRunSummary, SearchStage};
use crate::{
    design::Design,
    optimize::{
        candidate::{
            ScoredStone, SearchContext, measure_with_tone_cancellable, shape_penalty_for_planes,
            yield_loss_pct,
        },
        objective::{ObjectiveComponents, ObjectiveFidelity, ToneGoal},
        options::{MastChange, OptimizeCandidate, OptimizeResult},
    },
};
use pleochroma::{
    color::metrics::FaceUpTone,
    geometry::{meet_solver::MeetConstraint, stone_metrics::build_solid_mesh_and_measure},
};

#[cfg(doc)]
use super::{OptimizeOptions, baseline_report, optimize_design, optimize_design_with};

/// One design measured at [`ObjectiveFidelity::Full`].
struct ScoredState {
    after: ObjectiveComponents,
    score: f32,
    yield_loss_pct: f32,
    tone: Option<FaceUpTone>,
}

/// Solves `state` once (the gate that validates it hands the solve back), scores it at
/// [`ObjectiveFidelity::Full`] and blends the yield in -- the measurement [`build_result`]
/// gates every candidate on. `None` if `state` no longer solves (only a state that was
/// never accepted by the search could not), or, with `allow_cancel`, when
/// [`SearchHooks::cancel`] stopped the `Full` sweep (the partial sweep is dropped, never
/// scored). The accepted incumbent passes `allow_cancel: false`: it is finished even after
/// a cancellation.
///
/// Reports [`SearchStage::FinalFull`] through `hooks` (evaluations `evaluations`,
/// unchanged by this call -- same reasoning as [`baseline_report`]'s own report)
/// before running the scoring -- this call is as expensive as the baseline's;
/// without this report, a caller's progress ticker would stay stuck on its last
/// coordinate/polish reading through the whole final scoring, reading as the run
/// having already finished when it had not.
fn score_state(
    state: &Design,
    ctx: &SearchContext,
    hooks: &SearchHooks<'_>,
    evaluations: usize,
    allow_cancel: bool,
) -> Option<ScoredState> {
    hooks.report(evaluations, SearchStage::FinalFull);
    let feasible = ctx.space.feasible.as_ref()?;
    // Finish the one accepted incumbent even after cancellation, preserving the
    // existing best-so-far contract. The caller skips remaining rivals on Cancel.
    let final_check = feasible.baseline.telemetry.phase(6);
    // The validating solve is the only solve: its tiers are those of `state` as passed.
    let (_, solved) = feasible
        .baseline
        .validate_and_solve_with(state, feasible.policy, None)
        .ok()?;
    let planes = state.planes_from_solved(&solved);
    let (pleochroma::geometry::stone_metrics::SolidStatus::Closed(mesh), metrics) =
        build_solid_mesh_and_measure(&planes)
    else {
        return None;
    };
    if ctx
        .guard
        .is_some_and(|guard| !guard.admits(state, &solved, &planes, &mesh, metrics.as_ref()))
    {
        return None;
    }
    let warnings = crate::manufacturability::check_manufacturability_with_geometry(
        state,
        &solved,
        Some(&mesh),
        metrics.as_ref(),
        crate::manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    );
    if crate::optimize::candidate::manufacturability_regressed(ctx.baseline_warnings, &warnings) {
        return None;
    }
    drop(final_check);
    let stone = ScoredStone::of(state, &solved, &planes);
    let optics = feasible.baseline.telemetry.phase(5);
    let (after, tone, tone_loss) = measure_with_tone_cancellable(
        stone.finished(),
        ctx.material,
        ctx.weights,
        ObjectiveFidelity::Full,
        ctx.lighting,
        if allow_cancel { hooks.cancel } else { None },
    )?;
    drop(optics);
    let loss = yield_loss_pct(
        state,
        stone.yield_metrics(&planes, metrics.as_ref()).as_ref(),
    );
    let mut score = ctx.weights.score_with_tone(&after, loss, tone_loss);
    if ctx.shape_target.is_some() {
        score += shape_penalty_for_planes(ctx.shape_target, metrics.as_ref(), &planes);
    }
    if !score.is_finite() {
        return None;
    }
    Some(ScoredState {
        after,
        score,
        yield_loss_pct: loss,
        tone,
    })
}

/// Whether a tier's angle was edited. Both values come from the same stored `f64`, so an
/// untouched tier compares bit-equal and any difference is a real edit.
#[expect(
    clippy::float_cmp,
    reason = "exact comparison of an unedited copy of the same stored angle"
)]
const fn angle_differs(before: f64, after: f64) -> bool {
    before != after
}

/// Every tier whose angle differs between `original` and `state`, except the tiers that
/// follow a relation: their angle is the result of the tiers they read, so applying the
/// change of those moves them, and listing them too would ask for a direct change of an
/// angle the relation owns.
fn angle_changes(original: &Design, state: &Design) -> Vec<AngleChange> {
    original
        .tiers
        .iter()
        .zip(&state.tiers)
        .enumerate()
        .filter_map(|(index, (before_tier, after_tier))| {
            (angle_differs(before_tier.angle_deg, after_tier.angle_deg)
                && !original.is_tier_driven(index))
            .then_some(AngleChange {
                index,
                from_deg: before_tier.angle_deg,
                to_deg: after_tier.angle_deg,
            })
        })
        .collect()
}

/// Every `ScaleReference` tier whose mast differs between `original` and `state`.
fn mast_changes(original: &Design, state: &Design) -> Vec<MastChange> {
    original
        .tiers
        .iter()
        .zip(&state.tiers)
        .enumerate()
        .filter_map(|(index, (before_tier, after_tier))| {
            match (&before_tier.constraint, &after_tier.constraint) {
                (MeetConstraint::ScaleReference(from), MeetConstraint::ScaleReference(to))
                    if from.to_bits() != to.to_bits() =>
                {
                    Some(MastChange {
                        index,
                        from_mast: *from,
                        to_mast: *to,
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// `state` as a ranked alternative to `original`, or `None` when it changes nothing.
fn candidate_from(
    original: &Design,
    state: &Design,
    scored: &ScoredState,
    ctx: &SearchContext,
) -> Option<OptimizeCandidate> {
    let changes = angle_changes(original, state);
    let masts = mast_changes(original, state);
    if changes.is_empty() && masts.is_empty() {
        return None;
    }
    Some(OptimizeCandidate {
        provenance: None,
        preservation_baseline: ctx.space.feasible.as_ref().map(|f| f.baseline.clone()),
        changes,
        mast_changes: masts,
        after: scored.after,
        score: scored.score,
        yield_loss_pct: scored.yield_loss_pct,
        tone: scored.tone,
    })
}

/// The tone guarantee of a tone-preset run: a candidate whose tone moved against the
/// goal versus the start (paler under `Deeper`, darker under `Lighter`) never qualifies,
/// whatever its weighted score. No goal (tone weight 0) or an unmeasured tone passes.
fn tone_gate_passes(
    goal: Option<ToneGoal>,
    before: Option<FaceUpTone>,
    after: Option<FaceUpTone>,
) -> bool {
    match (goal, before, after) {
        (Some(goal), Some(before), Some(after)) => goal.not_worse(&before, &after),
        _ => true,
    }
}

/// [`optimize_design_with`]'s "measure the ending point" half: re-solves `current`
/// (see [`optimize_design`]'s own `# Panics` section for why the `.expect()` inside
/// this is safe) and the states in `rivals` it needs, scores each at
/// [`ObjectiveFidelity::Full`] (see [`score_state`]), and diffs its tier angles and
/// masts against `design`'s original ones to build the [`OptimizeCandidate`] list.
///
/// A state becomes a candidate when it changes something and its `Full` score is no
/// worse than the starting design's -- the search accepts candidates on the `Fast`
/// objective and the `Full` measurement here is the gate. The end point `current` is
/// always scored. The `rivals` (the pool's other states, best fast score first) are
/// scored one by one only while fewer than `keep` candidates have passed the gate, so a
/// run whose states pass pays exactly `keep` scorings and a run whose end point fails
/// tries the pool's fallbacks (see [`OptimizeOptions::pool_capacity`]) before it gives
/// up. No rival is scored once the search was cancelled; a cancel that arrives between
/// two scorings drops the rest and marks the outcome cancelled. Candidates are ranked by
/// their `Full` score (ties keep the end point first), and the outcome describes the
/// best one. When no state qualifies the outcome proposes no change: the design is
/// reported as it is, with the evaluations that were spent.
///
/// The `Fast` and `Full` figures can disagree outright, not just in detail: `Fast` is
/// the table-up pose alone and `Full` the mean over the tilt sweep, so a pavilion turned
/// past the critical angle can halve the face-up windowing while the sweep's mean, which
/// the extreme tilts dominate, barely moves or rises. The fallbacks are what keeps such a
/// run honest without making the search itself pay for `Full` scorings.
#[expect(
    clippy::too_many_arguments,
    reason = "the finish of one search: its inputs are the search's own pieces, bundled already"
)]
pub(super) fn build_result(
    design: &Design,
    current: &Design,
    rivals: &[Design],
    keep: usize,
    ctx: &SearchContext,
    baseline: &Baseline,
    hooks: &SearchHooks<'_>,
    summary: &SearchRunSummary,
) -> OptimizeResult {
    let keep = keep.max(1);
    let tone_gate = (ctx.weights.tone_weight > 0.0).then_some(ctx.weights.tone_goal);
    let passes = |state: &ScoredState| {
        state.score <= baseline.before_score
            && tone_gate_passes(tone_gate, baseline.tone_before, state.tone)
    };
    let mut candidates: Vec<OptimizeCandidate> = Vec::with_capacity(keep);
    if let Some(end_point) = score_state(current, ctx, hooks, summary.evaluations, false)
        && passes(&end_point)
        && let Some(candidate) = candidate_from(design, current, &end_point, ctx)
    {
        candidates.push(candidate);
    }
    let mut cancelled = summary.cancelled;
    if !cancelled {
        for rival in rivals {
            if candidates.len() >= keep {
                break;
            }
            if hooks.is_cancelled() {
                cancelled = true;
                break;
            }
            let scored = score_state(rival, ctx, hooks, summary.evaluations, true);
            // A rival whose `Full` sweep was stopped scores nothing: the cancel flag is never
            // cleared, so a `None` that finds it set is a cancelled scoring, like the check above.
            if scored.is_none() && hooks.is_cancelled() {
                cancelled = true;
                break;
            }
            if let Some(state) = scored
                && passes(&state)
                && let Some(candidate) = candidate_from(design, rival, &state, ctx)
            {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by(|a, b| a.score.total_cmp(&b.score));

    let tone_goal = tone_gate;
    let Some(best) = candidates.first() else {
        let mut unchanged = OptimizeResult::unchanged(
            OptimizeOutcome {
                provenance: None,
                before: baseline.before,
                before_score: baseline.before_score,
                before_yield_loss_pct: baseline.before_yield_loss_pct,
                after: baseline.before,
                after_score: baseline.before_score,
                after_yield_loss_pct: baseline.before_yield_loss_pct,
                evaluations: summary.evaluations,
                changes: Vec::new(),
                cancelled,
                polish_evaluations: summary.polish_evaluations,
                polish_improvement: 0.0,
            },
            ctx.lighting,
            baseline.tone_before,
            tone_goal,
        );
        unchanged.starts_run = summary.starts_run;
        unchanged.best_start = summary.best_start;
        return unchanged;
    };
    let outcome = OptimizeOutcome {
        provenance: None,
        before: baseline.before,
        before_score: baseline.before_score,
        before_yield_loss_pct: baseline.before_yield_loss_pct,
        after: best.after,
        after_score: best.score,
        after_yield_loss_pct: best.yield_loss_pct,
        evaluations: summary.evaluations,
        changes: best.changes.clone(),
        cancelled,
        polish_evaluations: summary.polish_evaluations,
        polish_improvement: summary.polish_improvement,
    };
    let best_mast_changes = best.mast_changes.clone();
    OptimizeResult {
        outcome,
        mast_changes: best_mast_changes,
        candidates,
        tone_before: baseline.tone_before,
        tone_goal,
        lighting: ctx.lighting,
        starts_run: summary.starts_run,
        best_start: summary.best_start,
    }
}

#[cfg(test)]
#[path = "result_tests.rs"]
mod tests;
