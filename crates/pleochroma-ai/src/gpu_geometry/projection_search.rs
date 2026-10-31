//! Bound-aware numerical probes for the cached projection. No acceptance policy
//! lives here: every returned design still needs the original preservation gate.
use super::{
    Design, GatePhase, MeetConstraint, Projection, ProjectionMissReason, TrustRegion,
    projection_step,
};
use std::collections::BTreeMap;

enum LineSearch {
    Certified(Box<Design>),
    Improved(Box<Design>, Vec<f64>, Vec<f64>),
}

impl Projection {
    pub(super) fn correct(
        &self,
        seed: &Design,
        origin: &[f64],
        mut current: Vec<f64>,
        requested_travel: f64,
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Result<Design, ProjectionMissReason> {
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
        for _ in 0..12 {
            let columns = self.jacobian_columns(&iterate, &x, &current, bounds, &legal)?;
            let qr_started = std::time::Instant::now();
            let correction = projection_step::correction(&columns, &current, &limits);
            super::super::elapsed(&self.backend.counters.qr_ns, qr_started);
            let mut correction = correction.ok_or(ProjectionMissReason::InvalidCorrection)?;
            projection_step::bound_correction(&mut correction, &x, origin, &limits, &legal)
                .ok_or(ProjectionMissReason::NoFeasibleDirection)?;
            let before = current.iter().map(|r| r * r).sum::<f64>();
            match self.search_line(&iterate, &x, &correction, before, requested_travel, trust)? {
                LineSearch::Certified(candidate) => return Ok(*candidate),
                LineSearch::Improved(candidate, next, rows) => {
                    (iterate, x, current) = (*candidate, next, rows);
                }
            }
        }
        let candidate = self
            .materialize(&iterate, &x, bounds)
            .ok_or(ProjectionMissReason::InvalidSeed)?;
        self.valid(&candidate, requested_travel)?;
        Ok(candidate)
    }

    fn search_line(
        &self,
        seed: &Design,
        x: &[f64],
        correction: &[f64],
        before: f64,
        requested_travel: f64,
        trust: TrustRegion<'_>,
    ) -> Result<LineSearch, ProjectionMissReason> {
        let mut saw_candidate = false;
        for start in (0..projection_step::BACKTRACK_STEPS).step_by(4) {
            let (probes, values) = self.line_candidates(seed, x, correction, trust, start);
            saw_candidate |= !probes.is_empty();
            let rows = self.batch(&probes, Some(requested_travel))?;
            for ((probe, next), r) in probes.into_iter().zip(values).zip(rows) {
                if self.gate_probe(&probe, requested_travel, &r, GatePhase::LineTrial)? {
                    return Ok(LineSearch::Certified(Box::new(probe)));
                }
                let Some(r) = r.rows else {
                    continue;
                };
                if r.iter().map(|v| v * v).sum::<f64>() < before {
                    return Ok(LineSearch::Improved(Box::new(probe), next, r));
                }
            }
        }
        Err(if saw_candidate {
            ProjectionMissReason::NoResidualProgress
        } else {
            ProjectionMissReason::NoFeasibleCandidate
        })
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
