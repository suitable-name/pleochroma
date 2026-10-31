//! The finite-difference teacher: what the simulator can say about which way to move.
//!
//! The simulator is deterministic, so instead of inferring a direction from the
//! noisy scalar reward of a proposal, the trainer asks it directly. [`EnvCore::teacher_label`]
//! probes every free tier with feasible two-sided evaluations where possible and reports
//! how each of the seven score components moves per degree of that tier
//! ([`TierSensitivity`]). A model that predicts those sensitivities can compose the direction
//! for ANY weighting of the objective from the weight fractions in the observation, so one
//! model serves every preset.
//!
//! # Components
//!
//! Everywhere (labels, model heads, loss) the seven components come in this order, all as
//! LOSSES (lower is better), in percentage points:
//!
//! | index | constant | figure |
//! |---|---|---|
//! | 0 | [`COMPONENT_WINDOWING`] | windowing percentage |
//! | 1 | [`COMPONENT_EXTINCTION`] | extinction percentage |
//! | 2 | [`COMPONENT_BRILLIANCE_LOSS`] | `100 - tilt_brilliance_pct` |
//! | 3 | [`COMPONENT_YIELD_LOSS`] | `100 - yield_pct` (NaN when yield is not measurable) |
//! | 4 | [`COMPONENT_LIGHTER_TONE_LOSS`] | shared lighter-tone loss |
//! | 5 | [`COMPONENT_DEEPER_TONE_LOSS`] | shared deeper-tone loss |
//! | 6 | [`COMPONENT_SHAPE_DISTANCE`] | `100 *` configured shape distance (NaN when absent) |
//!
//! A slope is the change of such a figure per degree of one tier, in percentage points per
//! degree; the score slope is in score points per degree. A negative slope means "increasing
//! this tier's signed angle lowers the figure".
//!
//! # From slopes to an action
//!
//! [`direction_from_slopes`] turns slopes into the pre-squash direction the policy mean uses:
//! the steepest tier gets `-MEAN_BOUND` times the sign of its slope and the others are
//! proportional. [`action_from_direction`] squashes it into degrees (`tanh(d) * step`), so the
//! steepest tier moves 95 % of the step limit. These formulas define the teacher's gradient
//! candidates and the sensitivity portion of the model mean; its learned residual is separate.
//!
//! # Probes and verified behavior
//!
//! Independent per-tier probes retain their measured sensitivities AND their best improving
//! coordinate action. The label finalizer compares those actions with canonical finite raw
//! gradient-scale candidates. If none improves, at most eight attempts on two tiers search
//! shorter/larger coordinate radii and two paired directions. This bounded search does not
//! prove optimality or cover every coupled escape direction.
//!
//! This split permits probe-level scheduling without losing coordinate improvements when
//! central differences cancel or a simultaneous gradient move is infeasible. A separate
//! behavior target teaches such moves without falsifying sensitivity labels. Every target
//! uses exactly the raw action measured by the teacher, at unit execution scale.
//! [`assemble_label`] and [`OracleOutcome`] remain the legacy slope-only interfaces.

use super::{
    action::ContinuousTierAction,
    common::{DesignEval, EnvCore, apply_action, yield_loss_pct},
    stepping::{candidates, usable_scales},
};

#[path = "teacher_search.rs"]
mod search;
pub use search::TeacherLabelWork;
#[path = "teacher_probe_work.rs"]
mod probe_work;
pub use probe_work::TierProbeWork;

/// Number of score components a sensitivity vector holds.
pub const N_COMPONENTS: usize = 7;
/// Index of the windowing percentage in a sensitivity vector.
pub const COMPONENT_WINDOWING: usize = 0;
/// Index of the extinction percentage in a sensitivity vector.
pub const COMPONENT_EXTINCTION: usize = 1;
/// Index of the brilliance loss (`100 - tilt_brilliance_pct`) in a sensitivity vector.
pub const COMPONENT_BRILLIANCE_LOSS: usize = 2;
/// Index of the yield loss (`100 - yield_pct`) in a sensitivity vector.
pub const COMPONENT_YIELD_LOSS: usize = 3;
/// Index of the lighter-tone loss component.
pub const COMPONENT_LIGHTER_TONE_LOSS: usize = 4;
/// Index of the deeper-tone loss component.
pub const COMPONENT_DEEPER_TONE_LOSS: usize = 5;
/// Index of the shape-distance component (`100 * distance`).
pub const COMPONENT_SHAPE_DISTANCE: usize = 6;

/// `atanh(0.95)`: the pre-squash magnitude of the steepest tier of a composed direction.
pub const MEAN_BOUND: f32 = 1.831_78;
/// Minimum score-slope scale (points/degree) before composing a policy direction.
pub const SLOPE_NORM_FLOOR: f32 = 0.05;
/// Compatibility constant retained for source users; direction normalization uses the floor.
pub const SLOPE_EPS: f32 = 1e-6;
/// Default line-search scales of a step, tried in this order.
pub const DEFAULT_LINE_SEARCH: [f64; 3] = [1.0, 0.5, 0.25];

/// One free tier's measured slopes at the current design, per degree of that tier.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierSensitivity {
    /// Component slopes, percentage points per degree, in the order of the `COMPONENT_*`
    /// constants; `components[3]` is NaN when the yield is not measurable for this design
    /// (no preform volume).
    pub components: [f32; N_COMPONENTS],
    /// The score slope, points per degree (the weighted composition of `components` as the
    /// environment scores it; measured directly, so it includes the score's own rounding).
    pub score: f32,
    /// Whether a positive-side probe contributed (`false` means a negative-only fallback).
    pub forward_probe: bool,
}

/// What the teacher measures at one state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TeacherLabel {
    /// One entry per tier of the design; `None` for a tier the environment does not move or
    /// whose probes both failed.
    pub tiers: Vec<Option<TierSensitivity>>,
    /// Pre-squash raw action of a verified improving candidate, one entry per tier.
    ///
    /// Apply with `action_from_direction` at the environment step limit and unit scale.
    /// `None` means the bounded search found no improving action, not proof of optimality.
    pub action_target: Option<Vec<f32>>,
    /// Score drop of the best measured target (points, >= 0), `None` when no search
    /// was requested or possible. `Some(0.0)` means the bounded search found no improvement.
    pub oracle_gain: Option<f32>,
    /// Nominal gradient-search scale, or one for a coordinate/paired target; `None` without a winner.
    ///
    /// This is provenance only: replay `action_target` at unit scale, not this scale.
    pub oracle_scale: Option<f32>,
    /// Candidate evaluations spent on probes, gradient scales and bounded rescue.
    pub evaluations: usize,
}

/// The probe of one tier: its slopes and what measuring them cost.
///
/// The unit of work of the teacher. A trainer that labels many designs makes one task of each
/// probe and joins them with the environment's `teacher_label_from_probes` method.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TierProbe {
    /// The tier's slopes, `None` when the tier is not a free tier of the environment, the probe
    /// size is not usable, or both the `+h` and the `-h` candidate were refused.
    pub sensitivity: Option<TierSensitivity>,
    /// Best improving canonical coordinate action measured by these probes, if any.
    /// It is independent of the central derivative, which can cancel at a local maximum.
    pub best_action: Option<CoordinateProbe>,
    /// Candidate evaluations spent: every probe candidate that reached the scorer counts, a
    /// candidate the environment refuses before scoring is free. `0` for a tier that is not
    /// probed at all.
    pub evaluations: usize,
}

/// An improving coordinate candidate measured in the unchanged source environment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinateProbe {
    /// Tier whose requested angle changes; preservation may adjust coupled coordinates.
    pub tier: usize,
    /// Finite pre-squash action, bounded by `MEAN_BOUND`.
    pub raw: f32,
    /// Measured score decrease after preservation and all validity gates.
    pub gain: f32,
}

/// The oracle step of a label: the steepest-descent action of the score slopes, tried at the
/// line-search scales in order, without changing the environment.
///
/// The default value is "no oracle step" (no figure, no evaluation): the answer for empty or
/// unusable scales, slopes of the wrong length and slopes with no usable entry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OracleOutcome {
    /// Score drop of the first accepted scale (points, > 0 for an accepted candidate),
    /// `Some(0.0)` when no scale improved, `None` when there was no oracle step.
    pub gain: Option<f32>,
    /// The scale that produced `gain`, `None` when nothing improved or there was no step.
    pub scale: Option<f32>,
    /// Candidate evaluations spent on the oracle scales.
    pub evaluations: usize,
}

/// The pre-squash direction of slopes `score` (points per degree, `None` = no slope):
/// `-MEAN_BOUND * (s_k / max(max_j |s_j|, SLOPE_NORM_FLOOR))`, `0.0` for a tier without a slope.
///
/// A slope that is NaN or infinite counts as "no slope" (it is excluded from the maximum and
/// its entry is `0.0`). When no tier has a slope every entry is `0.0`. The output is never
/// NaN, and lies in `[-MEAN_BOUND, MEAN_BOUND]` for every finite slope. Slopes are divided by
/// the finite maximum before multiplication, so even very large finite values stay bounded.
#[must_use]
pub fn direction_from_slopes(score: &[Option<f32>]) -> Vec<f32> {
    let usable = |slope: &Option<f32>| slope.filter(|value| value.is_finite());
    let steepest = score
        .iter()
        .filter_map(usable)
        .fold(0.0_f32, |max, slope| max.max(slope.abs()))
        .max(SLOPE_NORM_FLOOR);
    score
        .iter()
        .map(|slope| usable(slope).map_or(0.0, |slope| -MEAN_BOUND * (slope / steepest)))
        .collect()
}

/// `tanh(direction) * max_step_deg` per tier, the action the oracle (and the model) proposes.
///
/// The result has one delta per entry of `direction`, in the same order.
#[must_use]
pub fn action_from_direction(direction: &[f32], max_step_deg: f64) -> ContinuousTierAction {
    ContinuousTierAction::new(
        direction
            .iter()
            .map(|&raw| f64::from(raw.tanh()) * max_step_deg)
            .collect(),
    )
}

/// The score slope of every tier of a label (`None` for a tier without a measurement): the
/// input of the oracle step ([`RetargetEnv::oracle_outcome`](super::RetargetEnv::oracle_outcome)
/// and its twin on the optimizer environment).
#[must_use]
pub fn score_slopes(tiers: &[Option<TierSensitivity>]) -> Vec<Option<f32>> {
    tiers
        .iter()
        .map(|tier| tier.map(|sensitivity| sensitivity.score))
        .collect()
}

/// Joins the pieces of a label: the per-tier probes, the evaluations they cost and the oracle
/// step.
///
/// `tiers` holds one entry per tier of the design (`None` for a tier that was not probed or
/// whose probes failed), `probe_evaluations` the sum of the [`TierProbe::evaluations`] that
/// produced them. The label's `evaluations` is `probe_evaluations + oracle.evaluations`.
/// Nothing is recomputed. This legacy slope-only assembly has no behavior target; use the
/// environment's `teacher_label_from_probes` to retain coordinate winners and bounded rescue.
#[must_use]
pub const fn assemble_label(
    tiers: Vec<Option<TierSensitivity>>,
    probe_evaluations: usize,
    oracle: OracleOutcome,
) -> TeacherLabel {
    TeacherLabel {
        tiers,
        action_target: None,
        oracle_gain: oracle.gain,
        oracle_scale: oracle.scale,
        evaluations: probe_evaluations + oracle.evaluations,
    }
}

/// The seven component figures of `eval`, in the order of the `COMPONENT_*` constants.
fn component_values(eval: &DesignEval) -> [f32; N_COMPONENTS] {
    [
        eval.components.windowing_pct,
        eval.components.extinction_pct,
        100.0 - eval.components.tilt_brilliance_pct,
        eval.yield_pct.map_or(f32::NAN, yield_loss_pct),
        eval.tone_lighter_loss_pct.unwrap_or(f32::NAN),
        eval.tone_deeper_loss_pct.unwrap_or(f32::NAN),
        eval.shape_distance_pct,
    ]
}

impl EnvCore {
    /// What the teacher measures at the current design.
    ///
    /// For each free tier, candidates are built as a step builds them ([`apply_action`], so
    /// hinge re-anchoring and tier relations apply) and evaluated at configured fidelity. Feasible
    /// probes are taken on both sides when possible; clipped one-sided probes are used at bounds.
    /// Two-sided labels use a nonuniform three-point derivative about the current score. The
    /// denominator is the actual requested input displacement after safe/user-bound clipping,
    /// not a post-projection coordinate delta. A full projected no-op is not scored. Lower is
    /// better, so a negative score slope means "increase this tier's signed angle". Yield is NaN
    /// when unavailable.
    ///
    /// The finalizer retains the best verified coordinate or canonical gradient-scale
    /// candidate. If neither improves, it tries at most eight additional coordinate or paired actions.
    /// Empty or unusable `scales` asks for sensitivities only, unlike `step_scaled` where an
    /// empty list means unit scale. This is exactly the serial composition of `probe_tier`
    /// and `teacher_label_from_probes_adaptive`.
    ///
    /// Pure: nothing changes and no step is counted. Cost is at most two evaluations per
    /// free tier plus `scales.len()` plus eight rescue attempts. Invalid `h_deg` returns an
    /// empty label. Identity candidates and pre-scoring refusals cost no evaluations.
    pub(crate) fn teacher_label(&self, h_deg: f64, scales: &[f64]) -> TeacherLabel {
        let mut probes = vec![TierProbe::default(); self.current_design.tiers.len()];
        for &tier in self.space.free() {
            probes[tier] = self.probe_tier(tier, h_deg);
        }
        self.teacher_label_from_probes_adaptive(h_deg, &probes, scales, &|| false)
    }

    /// Per-tier score sensitivity at the current design, in score points per degree: the
    /// score slopes of [`Self::teacher_label`] (without an oracle step).
    ///
    /// One entry per tier of the design. A tier the environment does not move is `None`, as is
    /// a free tier with no feasible scored probe. Pure, like the label.
    pub(crate) fn score_gradient_fd(&self, h_deg: f64) -> Vec<Option<f32>> {
        score_slopes(&self.teacher_label(h_deg, &[]).tiers)
    }

    /// The probe of one tier: the unit of work of [`Self::teacher_label`].
    ///
    /// The positive and negative probes for `tier`, measured against the current design (see
    /// [`Self::teacher_label`] for the derivative and gates). The result has `sensitivity: None` and `evaluations: 0` for a
    /// tier that is not a free tier of the environment (pinned, or an index beyond the
    /// design's tiers) and for an `h_deg` that is not finite and positive; it has
    /// `sensitivity: None` and the evaluations actually spent when neither side is usable. Pure:
    /// nothing in the core changes, and any number of threads may probe the
    /// same environment at once.
    pub(crate) fn probe_tier(&self, tier: usize, h_deg: f64) -> TierProbe {
        if !(h_deg.is_finite() && h_deg > 0.0) || !self.policy_coordinate(tier).mobile {
            return TierProbe::default();
        }
        let mut evaluations = 0;
        let (sensitivity, best_action) = self.tier_sensitivity(tier, h_deg, &mut evaluations);
        TierProbe {
            sensitivity,
            best_action,
            evaluations,
        }
    }

    /// The oracle step for the given score slopes: [`direction_from_slopes`] of `slopes`,
    /// [`action_from_direction`] at the step limit, then `scales` in order through the same
    /// candidate helper [`EnvCore::step_scaled`] uses, without changing the core.
    ///
    /// `slopes` has one entry per tier of the design ([`score_slopes`] of a label's tiers).
    /// The first accepted scale gives `gain` (the current score minus the candidate's) and
    /// `scale`; when no scale is accepted the gain is `Some(0.0)` and the scale `None`. The
    /// default outcome (`None`, `None`, no evaluation) is returned for empty or unusable
    /// `scales` (see [`usable_scales`]; unlike a step, the oracle then takes no unit scale),
    /// for a `slopes` of any other length than the tier count, and when no entry is a finite
    /// slope. Pure, like [`Self::probe_tier`].
    pub(crate) fn oracle_outcome(&self, slopes: &[Option<f32>], scales: &[f64]) -> OracleOutcome {
        self.oracle_outcome_with(slopes, scales, false)
    }

    /// [`Self::oracle_outcome`], optionally evaluating the candidates of the line search
    /// concurrently (`parallel`, see [`EnvCore::try_scales_parallel`]). The outcome is
    /// bit-identical, `evaluations` included: it counts the candidates the sequential search
    /// would have evaluated, up to and including the accepted one.
    pub(crate) fn oracle_outcome_with(
        &self,
        slopes: &[Option<f32>],
        scales: &[f64],
        parallel: bool,
    ) -> OracleOutcome {
        self.oracle_outcome_adaptive(slopes, scales, &|| parallel)
    }

    /// Rechecks spare capacity between scales and shares a remaining suffix through the
    /// installed Rayon pool. The outcome and evaluation count follow sequential order;
    /// speculative candidates after the first accepted scale may also be computed.
    pub(crate) fn oracle_outcome_adaptive(
        &self,
        slopes: &[Option<f32>],
        scales: &[f64],
        parallel: &(dyn Fn() -> bool + Sync),
    ) -> OracleOutcome {
        let usable: Vec<f64> = usable_scales(scales).collect();
        let has_slope = slopes.iter().flatten().any(|slope| slope.is_finite());
        if usable.is_empty() || !has_slope || slopes.len() != self.current_design.tiers.len() {
            return OracleOutcome::default();
        }
        let action =
            action_from_direction(&direction_from_slopes(slopes), self.settings.max_step_deg);
        let mut evaluations = 0;
        for (scale, outcome) in candidates(self, &action, &usable, parallel) {
            evaluations += usize::from(outcome.evaluated);
            if outcome.is_accepted()
                && let Some(score) = outcome.score()
            {
                return OracleOutcome {
                    gain: Some(self.current_eval.score - score),
                    scale: Some(scale as f32),
                    evaluations,
                };
            }
        }
        OracleOutcome {
            gain: Some(0.0),
            scale: None,
            evaluations,
        }
    }

    /// The slopes of `tier`: a nonuniform two-sided derivative when both probes pass, otherwise
    /// the available one-sided derivative; `None` when both fail.
    /// `evaluations` is incremented for every probe that reached the scorer.
    fn tier_sensitivity(
        &self,
        tier: usize,
        h_deg: f64,
        evaluations: &mut usize,
    ) -> (Option<TierSensitivity>, Option<CoordinateProbe>) {
        let (positive_raw, positive_delta) = self.canonical_probe(tier, h_deg);
        let (negative_raw, negative_delta) = self.canonical_probe(tier, -h_deg);
        let positive = if positive_delta > 0.0 {
            let (result, spent) = self.probe(tier, positive_delta);
            *evaluations += usize::from(spent);
            result.map(|eval| (eval, positive_delta))
        } else {
            None
        };
        let negative = if negative_delta < 0.0 {
            let (result, spent) = self.probe(tier, negative_delta);
            *evaluations += usize::from(spent);
            result.map(|eval| (eval, negative_delta.abs()))
        } else {
            None
        };
        self.reduce_tier_samples(tier, positive_raw, negative_raw, positive, negative)
    }

    fn reduce_tier_samples(
        &self,
        tier: usize,
        positive_raw: f32,
        negative_raw: f32,
        positive: Option<(DesignEval, f64)>,
        negative: Option<(DesignEval, f64)>,
    ) -> (Option<TierSensitivity>, Option<CoordinateProbe>) {
        let mut best_action = None;
        for (sample, raw) in [(&positive, positive_raw), (&negative, negative_raw)] {
            if let Some((eval, _)) = sample {
                let gain = self.current_eval.score - eval.score;
                if gain.is_finite()
                    && gain > 0.0
                    && best_action.is_none_or(|best: CoordinateProbe| gain > best.gain)
                {
                    best_action = Some(CoordinateProbe { tier, raw, gain });
                }
            }
        }
        let sensitivity = match (positive, negative) {
            (Some((plus, h_plus)), Some((minus, h_minus))) => {
                Some(self.two_sided_sensitivity(&plus, &minus, h_plus, h_minus))
            }
            (Some((plus, h)), None) => Some(self.sensitivity(&plus, h, true)),
            (None, Some((minus, h))) => Some(self.sensitivity(&minus, -h, false)),
            (None, None) => None,
        };
        (sensitivity, best_action)
    }

    /// Choose the finite raw input first, then measure precisely the action it executes.
    /// The secant denominator is this actual requested displacement, never the nominal h.
    fn canonical_probe(&self, tier: usize, requested: f64) -> (f32, f64) {
        if requested == 0.0 {
            return (0.0, 0.0);
        }
        let delta = self.probe_delta(tier, requested);
        let mut raw = search::raw_for_fraction(delta / self.settings.max_step_deg);
        // Inverse tanh and f32 rounding can put a nominal endpoint just beyond a bound.
        // Move toward zero, never enlarge the measured executable displacement.
        for _ in 0..8 {
            let executed = f64::from(raw.tanh()) * self.settings.max_step_deg;
            if executed.is_finite() && executed.abs() <= delta.abs() {
                return (raw, executed);
            }
            raw = if raw.is_sign_negative() {
                raw.next_up()
            } else {
                raw.next_down()
            };
        }
        (0.0, 0.0)
    }

    fn probe_delta(&self, tier: usize, requested: f64) -> f64 {
        let coordinate = self.policy_coordinate(tier);
        if !coordinate.mobile || requested.is_nan() {
            return 0.0;
        }
        let old = self.current_design.tiers[tier].angle_deg;
        let represented = f64::from(old as f32);
        let (safe_lo, safe_hi) = if old.is_sign_negative() {
            (
                -super::moves::MAX_SAFE_CANDIDATE_ANGLE_DEG,
                -super::moves::MIN_SAFE_CANDIDATE_ANGLE_DEG,
            )
        } else {
            (
                super::moves::MIN_SAFE_CANDIDATE_ANGLE_DEG,
                super::moves::MAX_SAFE_CANDIDATE_ANGLE_DEG,
            )
        };
        let user_bounds = self.space.requested_bounds(tier);
        // Intersect true f64 authoring room with the policy's f32 represented room.
        // A target then survives both model projection and the environment's bounds.
        let lo = (user_bounds.0.max(safe_lo) - old)
            .max(f64::from(coordinate.min).max(safe_lo) - represented)
            .max(-self.settings.max_step_deg);
        let hi = (user_bounds.1.min(safe_hi) - old)
            .min(f64::from(coordinate.max).min(safe_hi) - represented)
            .min(self.settings.max_step_deg);
        if lo > 0.0 || hi < 0.0 || lo > hi {
            return 0.0;
        }
        requested.clamp(lo, hi)
    }

    /// Evaluates the current design with `tier` moved by `delta_deg` (every other tier at
    /// rest). The first element is the measurement, or `None` when the candidate is refused:
    /// a step would reject it before scoring (unsafe angle, no usable mast, relations), it
    /// does not solve, close or score finitely, or it fails the girdle or manufacturability
    /// gate. The second says whether the candidate reached the scorer (it cost an evaluation).
    fn probe(&self, tier: usize, delta_deg: f64) -> (Option<DesignEval>, bool) {
        let mut deltas = vec![0.0; self.current_design.tiers.len()];
        deltas[tier] = delta_deg;
        // `probe_delta` already clipped the requested displacement. The projection must
        // have the same coupled repair radius as an actual step at this state.
        let Ok(candidate) = apply_action(
            &self.space,
            &self.current_design,
            &ContinuousTierAction::new(deltas),
            self.settings.max_step_deg,
        ) else {
            return (None, false);
        };
        let evaluated = candidate != self.current_design;
        let eval = if evaluated {
            let Ok(eval) = self.evaluate_candidate_design(&candidate) else {
                return (None, true);
            };
            eval
        } else {
            // A feasible projected identity has a known zero response. Keep it in the
            // secant rather than mistaking it for an infeasible side of the derivative.
            self.current_eval.clone()
        };
        let usable = self.girdle_gate(&eval) && self.manufacturing_gate(&eval);
        (usable.then_some(eval), evaluated)
    }

    /// The difference quotients of `probed` against the current design for a move of
    /// `delta_deg` (negative for the `-h` fallback, which makes the quotient
    /// `(current - after) / h`).
    fn sensitivity(
        &self,
        probed: &DesignEval,
        delta_deg: f64,
        forward_probe: bool,
    ) -> TierSensitivity {
        let current = component_values(&self.current_eval);
        let after = component_values(probed);
        let slope =
            |moved: f32, rest: f32| ((f64::from(moved) - f64::from(rest)) / delta_deg) as f32;
        TierSensitivity {
            components: std::array::from_fn(|index| slope(after[index], current[index])),
            score: slope(probed.score, self.current_eval.score),
            forward_probe,
        }
    }

    fn two_sided_sensitivity(
        &self,
        positive: &DesignEval,
        negative: &DesignEval,
        h_plus: f64,
        h_minus: f64,
    ) -> TierSensitivity {
        let current = component_values(&self.current_eval);
        let plus = component_values(positive);
        let minus = component_values(negative);
        let quotient = |f0: f32, fp: f32, fm: f32| nonuniform_secant(f0, fp, fm, h_plus, h_minus);
        TierSensitivity {
            components: std::array::from_fn(|i| quotient(current[i], plus[i], minus[i])),
            score: quotient(self.current_eval.score, positive.score, negative.score),
            forward_probe: true,
        }
    }
}

fn nonuniform_secant(center: f32, positive: f32, negative: f32, h_plus: f64, h_minus: f64) -> f32 {
    if !(h_plus.is_finite() && h_minus.is_finite() && h_plus > 0.0 && h_minus > 0.0)
        || !center.is_finite()
        || !positive.is_finite()
        || !negative.is_finite()
    {
        return f32::NAN;
    }
    let span = h_plus + h_minus;
    let f0 = f64::from(center);
    let fp = f64::from(positive);
    let fm = f64::from(negative);
    ((h_minus / span) * ((fp - f0) / h_plus) + (h_plus / span) * ((f0 - fm) / h_minus)) as f32
}

#[cfg(test)]
#[path = "teacher_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "teacher_hotpath_tests.rs"]
mod hotpath_tests;
