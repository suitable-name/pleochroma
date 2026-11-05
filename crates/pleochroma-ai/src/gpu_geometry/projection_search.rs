//! Bound-aware numerical probes for the cached projection. Every returned design
//! still passes the original preservation gate.
use super::{
    Design, GateProbe, MeetConstraint, Projection, ProjectionMissReason, TrustRegion,
    projection_step,
};
use std::{collections::BTreeMap, sync::atomic::Ordering};

pub(super) enum LineSearch {
    Certified(Box<Design>),
    Improved(
        Box<Design>,
        Vec<f64>,
        Vec<f64>,
        super::Probe,
        Option<ProjectionMissReason>,
    ),
}

/// A non-improving trial retained for certification, with its squared residual norm
/// (`f64::INFINITY` when its rows were unusable).
pub(super) type DeferredTrial = (Design, f64);

/// At most this many deferred trials are certified per request, smallest residual
/// first. A failed request used to certify every retained trial in encounter order:
/// up to 24 backtracking steps per correction iteration and 12 iterations, about 52 ms
/// of CPU gate each and 95 % refused (throughput capture 2026-11-01: 47,850 deferred
/// trials, 54,704 line-trial gates, 2,837 CPU seconds). Returned designs are still
/// certified by the original gate; the cap only bounds how hard a failing request
/// searches before it reports its miss.
pub(super) const MAX_DEFERRED_CERTIFICATIONS: usize = 4;

/// Safety factor of the residual screen (`may_pass_gate`) over its derived limit.
///
/// Every state the original gate accepts has, for each junction, a point `q` within
/// `bound` (plus rounding) of every support plane and at most `bound` outside every
/// other plane: the necessary conditions of the outward-rounded prefilter
/// (`interval.cuh`). The residual rows use the basis point `p` instead. With unit
/// normals, `|p - q| <= 3 bound / |det|` for the basis determinant `det`, so every
/// incidence and containment row of an accepted state is at most
/// `bound (1 + 3/|det|) / scale`. The factor covers rounding, f32 production planes
/// and the change of `det` over a trust region; a screened trial keeps iterating and
/// the terminal iterate is still gated, so the screen never returns a design.
const SCREEN_SAFETY_FACTOR: f64 = 2.0;

impl Projection {
    pub(super) fn correct(
        &self,
        seed: &Design,
        origin: &[f64],
        mut current_probe: super::Probe,
        mut rejection: Option<ProjectionMissReason>,
        requested_travel: f64,
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Result<Design, ProjectionMissReason> {
        // The probe keeps its rows too: the terminal certification screens them.
        let mut current = current_probe
            .rows
            .clone()
            .ok_or(ProjectionMissReason::UnusableSeedResidual)?;
        let mut x = origin.to_vec();
        // Retain the actual materialized iterate. A normalized mast divided and
        // multiplied by scale need not reproduce its original f64 bits; using
        // this base lets materialize preserve every unchanged mast exactly.
        let mut iterate = seed.clone();
        let limits: Vec<_> = (0..x.len())
            .map(|i| if i < self.free.len() { 5.0 } else { 0.1 })
            .collect();
        let legal = self.parameter_bounds(origin, &limits, bounds);
        let trust = TrustRegion {
            origin,
            limits: &limits,
            bounds,
            legal: &legal,
        };
        // At most BACKTRACK_STEPS trials per iteration are retained. Keep their
        // exact authored states until a certified result is found or search ends:
        // residual progress is only a priority heuristic, never an invalidity proof.
        // It orders the terminal certification (`certify_deferred`).
        let mut deferred: Vec<DeferredTrial> = Vec::new();
        for _ in 0..12 {
            let columns = match self.jacobian_columns(&iterate, &x, &current, bounds, &legal) {
                Ok(columns) => columns,
                Err(reason) => {
                    return self.certify_terminal_or_deferred(
                        &iterate,
                        &current_probe,
                        rejection,
                        requested_travel,
                        reason,
                        &mut deferred,
                    );
                }
            };
            let qr_started = std::time::Instant::now();
            let step_result = projection_step::bounded_qr_correction(
                &columns, &current, &x, origin, &limits, &legal,
            );
            super::super::elapsed(&self.backend.counters.qr_ns, qr_started);
            let correction = match step_result {
                Ok(correction) => correction,
                Err(false) => {
                    return self.certify_terminal_or_deferred(
                        &iterate,
                        &current_probe,
                        rejection,
                        requested_travel,
                        ProjectionMissReason::InvalidCorrection,
                        &mut deferred,
                    );
                }
                Err(true) => {
                    return self.certify_terminal_or_deferred(
                        &iterate,
                        &current_probe,
                        rejection,
                        requested_travel,
                        ProjectionMissReason::NoFeasibleDirection,
                        &mut deferred,
                    );
                }
            };
            let before = current.iter().map(|r| r * r).sum::<f64>();
            let search = self.search_line(
                &iterate,
                &x,
                &correction,
                before,
                requested_travel,
                trust,
                &mut deferred,
            );
            match search {
                Ok(LineSearch::Certified(candidate)) => return Ok(*candidate),
                Ok(LineSearch::Improved(candidate, next, rows, probe, rejected)) => {
                    (iterate, x, current, current_probe, rejection) =
                        (*candidate, next, rows, probe, rejected);
                }
                Err(reason) => {
                    return self.certify_terminal_or_deferred(
                        &iterate,
                        &current_probe,
                        rejection,
                        requested_travel,
                        reason,
                        &mut deferred,
                    );
                }
            }
        }
        // `iterate` is the exact state whose gate outcome is retained above.
        self.certify_terminal_or_deferred(
            &iterate,
            &current_probe,
            rejection,
            requested_travel,
            rejection.unwrap_or(ProjectionMissReason::NoResidualProgress),
            &mut deferred,
        )
    }

    pub(super) fn certify_terminal_or_deferred(
        &self,
        candidate: &Design,
        probe: &super::Probe,
        rejection: Option<ProjectionMissReason>,
        requested_travel: f64,
        reason: ProjectionMissReason,
        deferred: &mut Vec<DeferredTrial>,
    ) -> Result<Design, ProjectionMissReason> {
        self.backend
            .check()
            .map_err(|_| ProjectionMissReason::ExecutionFailure)?;
        // A previously feasible trial must never mask cancellation or an unknown
        // infrastructure/gate failure from this request.
        if matches!(
            reason,
            ProjectionMissReason::ExecutionFailure
                | ProjectionMissReason::Cancelled
                | ProjectionMissReason::UnclassifiedGateFailure
        ) {
            return Err(reason);
        }
        if let Some(candidate) = self.certify_deferred(deferred, requested_travel)? {
            return Ok(candidate);
        }
        self.certify_terminal(candidate, probe, rejection, requested_travel, reason)
    }

    fn certify_deferred(
        &self,
        deferred: &mut Vec<DeferredTrial>,
        requested_travel: f64,
    ) -> Result<Option<Design>, ProjectionMissReason> {
        // All retained trials survived the conservative prefilter; even unusable or
        // flat numeric rows need the gate. Certify the smallest residuals first (a
        // stable sort keeps encounter order among equal norms, so the order stays
        // deterministic) and at most MAX_DEFERRED_CERTIFICATIONS of them.
        let mut trials = std::mem::take(deferred);
        trials.sort_by(|a, b| a.1.total_cmp(&b.1));
        let skipped = trials.len().saturating_sub(MAX_DEFERRED_CERTIFICATIONS);
        if skipped > 0 {
            self.backend.uncertified_deferred_trials(skipped as u64);
        }
        for (candidate, _) in trials.into_iter().take(MAX_DEFERRED_CERTIFICATIONS) {
            match self.valid_at(
                &candidate,
                requested_travel,
                super::GatePhase::DeferredTrial,
            ) {
                Ok(()) => return Ok(Some(candidate)),
                Err(
                    ProjectionMissReason::OriginalGateRejected
                    | ProjectionMissReason::RequestedLimitsRejected,
                ) => {}
                Err(reason) => return Err(reason),
            }
        }
        Ok(None)
    }

    pub(super) fn defer_line_trial(
        &self,
        deferred: &mut Vec<DeferredTrial>,
        candidate: Design,
        residual: f64,
    ) {
        // A request can revisit an identical authored trial across correction
        // iterations. Retain it once, independently of the small episode-wide
        // negative cache, whose entries may be evicted before terminal recovery.
        if !deferred
            .iter()
            .any(|(earlier, _)| super::super::certificate::same_state(earlier, &candidate))
        {
            self.backend.deferred_line_trial_gate();
            deferred.push((candidate, residual));
        }
    }

    pub(super) fn certify_terminal(
        &self,
        candidate: &Design,
        probe: &super::Probe,
        rejection: Option<ProjectionMissReason>,
        requested_travel: f64,
        reason: ProjectionMissReason,
    ) -> Result<Design, ProjectionMissReason> {
        self.backend
            .check()
            .map_err(|_| ProjectionMissReason::ExecutionFailure)?;
        if rejection.is_some() {
            return Err(reason);
        }
        if probe
            .rows
            .as_deref()
            .is_some_and(|rows| !self.may_pass_gate(rows))
        {
            self.backend.screened_gate();
            return Err(reason);
        }
        match self.gate_probe(
            candidate,
            requested_travel,
            probe,
            super::GatePhase::FinalIterate,
        )? {
            GateProbe::Accepted => Ok(candidate.clone()),
            GateProbe::Rejected(_) | GateProbe::Impossible => Err(reason),
        }
    }

    pub(super) fn search_line(
        &self,
        seed: &Design,
        x: &[f64],
        correction: &[f64],
        before: f64,
        requested_travel: f64,
        trust: TrustRegion<'_>,
        deferred: &mut Vec<DeferredTrial>,
    ) -> Result<LineSearch, ProjectionMissReason> {
        let mut saw_candidate = false;
        for start in (0..projection_step::BACKTRACK_STEPS).step_by(4) {
            let (probes, values) = self.line_candidates(seed, x, correction, trust, start);
            saw_candidate |= !probes.is_empty();
            let rows = self.batch(&probes, Some(requested_travel))?;
            for ((probe, next), residual_probe) in probes.into_iter().zip(values).zip(rows) {
                let norm = residual_probe
                    .rows
                    .as_ref()
                    .map_or(f64::INFINITY, |r| r.iter().map(|v| v * v).sum::<f64>());
                let improves = norm < before;
                if !improves {
                    if residual_probe.possible {
                        self.defer_line_trial(deferred, probe, norm);
                    } else {
                        // Only the outward-rounded proof can discard a trial.
                        self.backend.skipped_line_trial_gates(1);
                        self.backend
                            .counters
                            .prefilter
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    continue;
                }
                if residual_probe
                    .rows
                    .as_deref()
                    .is_some_and(|rows| !self.may_pass_gate(rows))
                {
                    // Progress, but still too far from every junction to pass: iterate on
                    // without a gate. Not a rejection, so the terminal iterate is checked.
                    self.backend.screened_gate();
                    let r = residual_probe
                        .rows
                        .clone()
                        .expect("improving trial has rows");
                    return Ok(LineSearch::Improved(
                        Box::new(probe),
                        next,
                        r,
                        residual_probe,
                        None,
                    ));
                }
                let rejection = match self.gate_probe(
                    &probe,
                    requested_travel,
                    &residual_probe,
                    super::GatePhase::LineTrial,
                )? {
                    GateProbe::Accepted => return Ok(LineSearch::Certified(Box::new(probe))),
                    GateProbe::Rejected(reason) => Some(reason),
                    GateProbe::Impossible => {
                        self.backend.skipped_line_trial_gates(1);
                        None
                    }
                };
                let r = residual_probe
                    .rows
                    .clone()
                    .expect("improving trial has rows");
                return Ok(LineSearch::Improved(
                    Box::new(probe),
                    next,
                    r,
                    residual_probe,
                    rejection,
                ));
            }
        }
        if let Some(candidate) = self.certify_deferred(deferred, requested_travel)? {
            return Ok(LineSearch::Certified(Box::new(candidate)));
        }
        Err(if saw_candidate {
            ProjectionMissReason::NoResidualProgress
        } else {
            ProjectionMissReason::NoFeasibleCandidate
        })
    }

    /// Whether a state with these residual `rows` can still pass the original gate:
    /// false only when some junction's incidence or containment row exceeds the limit
    /// derived at [`SCREEN_SAFETY_FACTOR`]. Rows of another shape are never screened.
    pub(super) fn may_pass_gate(&self, rows: &[f64]) -> bool {
        if rows.len() != self.network.rows {
            return true;
        }
        let mut at = 0;
        for junction in &self.network.junctions {
            let transfer = 1.0 + 3.0 / junction.basis_det.abs().max(1e-8);
            let limit = SCREEN_SAFETY_FACTOR * junction.bound / self.network.scale * transfer;
            let incidence = &rows[at..at + junction.targets.len()];
            // A row is the scaled residual minus the original's own miss.
            if incidence
                .iter()
                .zip(&junction.targets)
                .any(|(row, target)| (row + target).abs() > limit)
                || rows[at + junction.targets.len()] > limit
            {
                return false;
            }
            at += junction.targets.len() + 1;
        }
        true
    }

    pub(super) fn parameter_bounds(
        &self,
        origin: &[f64],
        limits: &[f64],
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Vec<(f64, f64)> {
        origin
            .iter()
            .zip(limits)
            .enumerate()
            .map(|(variable, (&value, &limit))| {
                let authored = if let Some(&tier) = self.free.get(variable) {
                    let safe = pleochroma_cut_core::optimize::MAX_SAFE_CANDIDATE_ANGLE_DEG;
                    let side = if self.original.tiers[tier].angle_deg.is_sign_negative() {
                        (-safe, 0.0_f64.next_down())
                    } else {
                        (0.0_f64.next_up(), safe)
                    };
                    let requested = bounds.get(&tier).copied().unwrap_or(side);
                    (requested.0.max(side.0), requested.1.min(side.1))
                } else {
                    (1e-9_f64.next_up(), f64::INFINITY)
                };
                (
                    (value - limit).max(authored.0),
                    (value + limit).min(authored.1),
                )
            })
            .collect()
    }

    fn parameter(&self, candidate: &Design, variable: usize) -> Option<f64> {
        if let Some(&tier) = self.free.get(variable) {
            return Some(candidate.tiers.get(tier)?.angle_deg);
        }
        let tier = *self.masts.get(variable.checked_sub(self.free.len())?)?;
        match candidate.tiers.get(tier)?.constraint {
            MeetConstraint::ScaleReference(mast) => Some(mast / self.network.scale),
            _ => None,
        }
    }

    fn difference_probe(
        &self,
        seed: &Design,
        x: &[f64],
        variable: usize,
        nominal: f64,
        bounds: &BTreeMap<usize, (f64, f64)>,
        legal: (f64, f64),
    ) -> Option<(Design, f64)> {
        let step = projection_step::difference_step(x[variable], nominal, legal)?;
        let mut next = x.to_vec();
        next[variable] += step;
        let candidate = self.materialize(seed, &next, bounds)?;
        // Mast multiplication and relation evaluation may round again. The
        // derivative's divisor is the change actually represented in the design.
        let actual = self.parameter(&candidate, variable)?;
        let represented = actual - x[variable];
        (represented.is_finite()
            && represented != 0.0
            && actual >= legal.0
            && actual <= legal.1
            && represented.is_sign_positive() == nominal.is_sign_positive())
        .then_some((candidate, represented))
    }

    pub(super) fn jacobian_columns(
        &self,
        seed: &Design,
        x: &[f64],
        current: &[f64],
        bounds: &BTreeMap<usize, (f64, f64)>,
        legal: &[(f64, f64)],
    ) -> Result<Vec<Vec<f64>>, ProjectionMissReason> {
        let nominal = |variable| {
            if variable < self.free.len() {
                2e-3
            } else {
                2e-5
            }
        };
        let mut probes = Vec::new();
        let mut variables = Vec::new();
        let mut columns = vec![vec![0.0; current.len()]; x.len()];
        for variable in 0..x.len() {
            for h in [nominal(variable), -nominal(variable)] {
                if let Some((probe, represented)) =
                    self.difference_probe(seed, x, variable, h, bounds, legal[variable])
                {
                    probes.push(probe);
                    variables.push((variable, represented));
                    break;
                }
            }
        }
        let mut inward = Vec::new();
        let mut inward_variables = Vec::new();
        for (offset, chunk) in probes.chunks(32).enumerate() {
            for (&(variable, step), probe) in variables[offset * 32..]
                .iter()
                .zip(self.batch(chunk, None)?)
            {
                if let Some(rows) = probe.rows {
                    if rows.len() != current.len() {
                        return Err(ProjectionMissReason::InvalidLinearization);
                    }
                    // A clipped probe can stay on the same production-f32 plane
                    // plateau. Try the opposite full-sized legal probe before
                    // deciding this direction has zero numerical sensitivity.
                    if rows.iter().zip(current).any(|(a, b)| a != b) {
                        columns[variable] = rows
                            .iter()
                            .zip(current)
                            .map(|(a, b)| (a - b) / step)
                            .collect();
                        continue;
                    }
                }
                if step > 0.0
                    && let Some((probe, represented)) = self.difference_probe(
                        seed,
                        x,
                        variable,
                        -nominal(variable),
                        bounds,
                        legal[variable],
                    )
                {
                    inward.push(probe);
                    inward_variables.push((variable, represented));
                }
            }
        }
        for ((variable, step), probe) in
            inward_variables.into_iter().zip(self.batch(&inward, None)?)
        {
            if let Some(rows) = probe.rows {
                if rows.len() != current.len() {
                    return Err(ProjectionMissReason::InvalidLinearization);
                }
                columns[variable] = rows
                    .iter()
                    .zip(current)
                    .map(|(a, b)| (a - b) / step)
                    .collect();
            }
        }
        if columns.iter().flatten().any(|value| !value.is_finite()) {
            return Err(ProjectionMissReason::InvalidLinearization);
        }
        Ok(columns)
    }

    pub(super) fn line_candidates(
        &self,
        seed: &Design,
        x: &[f64],
        correction: &[f64],
        trust: TrustRegion<'_>,
        start: usize,
    ) -> (Vec<Design>, Vec<Vec<f64>>) {
        let mut probes = Vec::new();
        let mut values: Vec<Vec<f64>> = Vec::new();
        for index in start..(start + 4).min(projection_step::BACKTRACK_STEPS) {
            let factor = projection_step::backtrack_factor(index);
            let next: Vec<_> = x
                .iter()
                .zip(correction)
                .map(|(v, dx)| v + factor * dx)
                .collect();
            if next
                .iter()
                .zip(trust.origin)
                .zip(trust.limits)
                .zip(trust.legal)
                .any(|(((a, b), limit), (lo, hi))| {
                    !a.is_finite() || (a - b).abs() > *limit || a < lo || a > hi
                })
            {
                continue;
            }
            if let Some(probe) = self.materialize(seed, &next, trust.bounds) {
                let Some(represented): Option<Vec<_>> =
                    (0..x.len()).map(|i| self.parameter(&probe, i)).collect()
                else {
                    continue;
                };
                if represented
                    .iter()
                    .zip(x)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
                    || represented
                        .iter()
                        .zip(trust.origin)
                        .zip(trust.limits)
                        .zip(trust.legal)
                        .any(|(((a, b), limit), (lo, hi))| {
                            !a.is_finite() || (a - b).abs() > *limit || a < lo || a > hi
                        })
                    || values.iter().any(|old| {
                        old.iter()
                            .zip(&represented)
                            .all(|(a, b)| a.to_bits() == b.to_bits())
                    })
                {
                    continue;
                }
                probes.push(probe);
                values.push(represented);
            }
        }
        (probes, values)
    }
}
