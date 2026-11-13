//! The three-phase pipeline itself: [`SolveContext::run_pipeline`], its
//! [`Origin`] bookkeeping, the phase-2 per-block least-squares
//! [`block_estimate`], and the [`PipelineResult`] it produces.

use std::collections::BTreeMap;

use super::{
    super::{
        EPS_INCIDENT, LEVEL_TOL, MAX_CONSTRUCTIVE_SWEEPS, MAX_REFINE_SWEEPS, MeetConstraint,
        MeetTierInput, SolveControl, SolveError, SolvePhase, SolveProgress,
        candidates::{
            CandidateVertex, SolvePlane, blank_planes, enumerate_candidate_vertices_cancellable,
            filter_levels_by_instance_support, group_levels,
        },
        phase1_cache::{CachedCandidate, Phase1Cache},
    },
    context::SolveContext,
};

/// A block or sweep count for [`SolveProgress`], saturating (the counts are tiny; the
/// saturation only keeps the conversion total).
fn progress_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

// NOTE: no annihilation guard ("a pick must not erase another facet's corners"). It
// presumes the surrounding facets are already correct, so from a wrong intermediate
// configuration it would entrench the wrong values.

/// How a tier's current value was last established, for strategy/detail reporting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Origin {
    /// Untouched (still at the scale prior).
    Unset,
    /// Given directly as a [`MeetConstraint::ScaleReference`].
    Anchor,
    /// Constructive pass, named-reference incidence.
    ConstructiveNamed,
    /// Constructive pass, rank-1 prior.
    ConstructiveRank1,
    /// Per-block estimate (phase 2), not yet snapped to a vertex.
    Estimated,
    /// Estimate subsequently snapped to a vertex level by the refinement sweeps.
    Refined,
}

/// Per-block least-squares fit of `mast ~ a*cos(theta) + b*sin(theta)` over the
/// already-settled tiers of one block, evaluated for a target tier. `theta` is
/// recovered per tier from its first instance normal. Falls back to a pure
/// `b*sin(theta)` fit, then to `fallback`, as data thins out.
fn block_estimate(
    members: &[(f64, f64, f64)], // (cos_theta, sin_theta, mast) of settled same-block tiers
    cos_t: f64,
    sin_t: f64,
    fallback: f64,
) -> f64 {
    // 2x2 normal equations for [a, b].
    if members.len() >= 2 {
        let (mut cc, mut cs, mut ss, mut cm, mut sm) = (0.0_f64, 0.0, 0.0, 0.0, 0.0);
        for &(c, s, m) in members {
            cc = c.mul_add(c, cc);
            cs = c.mul_add(s, cs);
            ss = s.mul_add(s, ss);
            cm = c.mul_add(m, cm);
            sm = s.mul_add(m, sm);
        }
        // Determinant of [[cc, cs], [cs, ss]] -- `cs * cs` is deliberate.
        let det = cs.mul_add(-cs, cc * ss);
        if det.abs() > 1e-9 {
            let a = sm.mul_add(-cs, cm * ss) / det;
            let b = cm.mul_add(-cs, sm * cc) / det;
            let est = a.mul_add(cos_t, b * sin_t);
            if est.is_finite() && est > 1e-6 {
                return est;
            }
        }
    }
    // b-only fit through the origin of the cos axis.
    let usable: Vec<&(f64, f64, f64)> = members.iter().filter(|(_, s, _)| *s > 0.1).collect();
    if !usable.is_empty() {
        let b = usable.iter().map(|(_, s, m)| m / s).sum::<f64>() / usable.len() as f64;
        let est = b * sin_t;
        if est.is_finite() && est > 1e-6 {
            return est;
        }
    }
    fallback
}

/// Everything one full pipeline run (phases 1-3) produces, before it is mapped to
/// per-tier [`super::super::SolvedTier`]s.
pub(in crate::geometry::meet_solver) struct PipelineResult {
    pub(in crate::geometry::meet_solver) mast: Vec<f64>,
    pub(super) origin: Vec<Origin>,
    pub(in crate::geometry::meet_solver) last_pick: Vec<Option<(usize, usize)>>,
    pub(super) named_cause: Vec<Option<&'static str>>,
    pub(super) refine_sweeps: usize,
    pub(super) converged: bool,
}

impl SolveContext<'_> {
    // Phase 1 walks tiers in strict file order (Gauss-Seidel). `.asc` order is normally
    // the cutting order, and each tier's meet points lie on the arrangement of the tiers
    // before it. A free-running order was rejected: it gave a worse per-design success rate.
    //
    // `overrides` (tier index -> vertex-level index) forces phase-1 picks, bypassing the
    // named rule and the rank-1 prior. Phase 3 then refines an overridden tier by nearest
    // level only. Empty overrides give the plain solve.
    // The knob the repair search of
    // [`solve_meet_points_verified`](super::super::solve_meet_points_verified) turns.
    // `anchor_values` (anchor tier index -> mast) replaces a `ScaleReference` value without
    // changing the tier list. Empty maps give the plain solve.
    //
    // `control` is checked once per constructive sweep, once per unsettled tier, once per
    // refinement sweep, and inside phase 3's candidate enumeration. An unused control never
    // cancels, so the result is the infallible one.
    #[expect(
        clippy::too_many_lines,
        reason = "three sequential phases of one algorithm (constructive pass, \
                  per-block estimate, nearest-level refinement -- see the module docs' \
                  walkthrough) sharing one set of per-tier working vectors (mast, \
                  origin, settled, last_pick); splitting the phases into separate \
                  functions would just turn those shared locals into a too-many-\
                  arguments (or a bespoke context struct) problem at each call boundary"
    )]
    pub(in crate::geometry::meet_solver) fn run_pipeline(
        &self,
        overrides: &BTreeMap<usize, usize>,
        anchor_values: &BTreeMap<usize, f64>,
        control: &SolveControl<'_>,
    ) -> Result<PipelineResult, SolveError> {
        let tiers: &[MeetTierInput] = self.tiers;
        let n = tiers.len();
        let normals = &self.normals;
        let blocks = &self.blocks;
        let resolved_named = &self.resolved_named;
        let is_anchor = &self.is_anchor;
        let scale_norm = self.scale_norm;
        // Every anchor mast (a real `ScaleReference` value or a
        // repair-search `anchor_values` override, both in the design's real
        // absolute units) is normalised into the same order-1 neighbourhood
        // `EPS_FEAS`/`EPS_INCIDENT`/`LEVEL_TOL`/`BLANK_HALF_EXTENT` assume,
        // before any candidate-vertex geometry runs; `to_solved` undoes this
        // by multiplying back by `scale_norm` once the pipeline is done (see
        // `SolveContext::scale_norm`'s doc comment). `scale_norm == 1.0`
        // whenever the design's own scale already rounds to `2^0`, and
        // dividing/multiplying by exactly `1.0` is a bit-exact no-op.
        let scale_prior = self.scale_prior / scale_norm;
        let domination_limit = self.domination_limit;
        let mut mast: Vec<f64> = tiers
            .iter()
            .enumerate()
            .map(|(i, t)| match &t.constraint {
                MeetConstraint::ScaleReference(v) => {
                    anchor_values.get(&i).copied().unwrap_or_else(|| v.abs()) / scale_norm
                }
                _ => scale_prior,
            })
            .collect();
        let mut origin: Vec<Origin> = is_anchor
            .iter()
            .map(|&a| if a { Origin::Anchor } else { Origin::Unset })
            .collect();
        let mut last_pick: Vec<Option<(usize, usize)>> = vec![None; n];
        // Why a tier with resolved named refs nevertheless settled on the rank-1
        // prior in phase 1, for the report's detail string.
        let mut named_cause: Vec<Option<&'static str>> = vec![None; n];

        // ---- Phase 1: constructive pass. ----
        let mut settled: Vec<bool> = is_anchor.clone();
        let mut named_release = false;
        // Incremental candidate-vertex cache for phase 1 only (see `Phase1Cache`'s
        // doc comment); seeded with whatever tiers start out settled (anchors).
        let mut phase1_cache = Phase1Cache::new();
        for (j, &s) in settled.iter().enumerate() {
            if s {
                phase1_cache.add_tier(j, &normals[j], mast[j]);
            }
        }
        let mut last_constructive_sweep = 1u32;
        for pass_idx in 0..MAX_CONSTRUCTIVE_SWEEPS {
            if control.is_cancelled() {
                return Err(SolveError::Cancelled);
            }
            last_constructive_sweep = progress_count(pass_idx + 1);
            control.report(SolveProgress {
                phase: SolvePhase::Constructive,
                sweep: last_constructive_sweep,
                max_sweeps: progress_count(MAX_CONSTRUCTIVE_SWEEPS),
                blocks_done: progress_count(settled.iter().filter(|&&s| s).count()),
                blocks_total: progress_count(n),
            });
            let mut progress = false;
            for i in 0..n {
                if settled[i] {
                    continue;
                }
                // Also checked per unsettled tier: one pass's filtering over all of them
                // can be long enough to matter for cancel latency.
                if control.is_cancelled() {
                    return Err(SolveError::Cancelled);
                }
                let refs = &resolved_named[i];
                let refs_settled: Vec<usize> =
                    refs.iter().copied().filter(|&t| settled[t]).collect();
                // A named tier waits for its references to settle (they usually
                // do, on a later pass); after a pass with no progress,
                // `named_release` lets it settle from geometry alone.
                if !refs.is_empty() && refs_settled.len() < refs.len() && !named_release {
                    continue;
                }

                // `phase1_cache` mirrors a fresh enumeration of blanks + every
                // settled tier's planes, kept incrementally (see its doc comment).
                let n0 = normals[i][0];
                let feasible: Vec<&CachedCandidate> = phase1_cache
                    .candidates
                    .iter()
                    .filter(|c| c.violated.is_none())
                    .collect();
                let vals: Vec<f64> = feasible.iter().map(|c| n0.dot(c.v)).collect();
                let levels = group_levels(vals.iter().copied().filter(|v| *v > 1e-9).collect());
                if levels.is_empty() {
                    continue;
                }

                // Named rule: shallowest level with a candidate incident to every settled
                // reference. All-or-nothing on purpose: a partial match is weak evidence that
                // the constraint does not hold there, and a best-partial variant performs worse.
                // Stores the *index* into `levels`, so `last_pick` needs no value re-search.
                let mut choice: Option<(usize, Origin)> = None;
                if let Some(&forced) = overrides.get(&i) {
                    // Repair-search override: force this tier's pick to the given
                    // level, bypassing both the named rule and the rank-1 prior.
                    choice = Some((forced.min(levels.len() - 1), Origin::ConstructiveRank1));
                } else if !refs_settled.is_empty() {
                    let found = levels.iter().position(|&head| {
                        feasible.iter().zip(&vals).any(|(c, &val)| {
                            (val - head).abs() <= LEVEL_TOL
                                && refs_settled.iter().all(|&t| {
                                    normals[t]
                                        .iter()
                                        .any(|&nr| (nr.dot(c.v) - mast[t]).abs() < EPS_INCIDENT)
                                })
                        })
                    });
                    if let Some(idx) = found {
                        choice = Some((idx, Origin::ConstructiveNamed));
                    }
                    // No rescue when no level is incident to all refs: weakening the
                    // acceptance test admits self-consistent wrong corners.
                }
                let (li, orig) = choice
                    .unwrap_or_else(|| (usize::from(levels.len() >= 2), Origin::ConstructiveRank1));
                let value = levels[li];
                if value.is_finite() && value > 1e-9 && value <= domination_limit {
                    if orig == Origin::ConstructiveRank1
                        && !refs.is_empty()
                        && !overrides.contains_key(&i)
                    {
                        named_cause[i] = Some(if refs_settled.len() < refs.len() {
                            "refs not yet settled at release"
                        } else {
                            "no feasible level incident to all settled refs"
                        });
                    }
                    mast[i] = value;
                    origin[i] = orig;
                    last_pick[i] = Some((li, levels.len()));
                    settled[i] = true;
                    phase1_cache.add_tier(i, &normals[i], mast[i]);
                    progress = true;
                }
            }

            if !progress {
                if named_release {
                    break;
                }
                named_release = true;
            } else if settled.iter().all(|&s| s) {
                break;
            }
        }
        // Final constructive-phase report: the loop above can `break` as soon
        // as every tier settles, mid-pass, without another iteration's
        // pre-pass report ever reflecting that -- so report the actual final
        // settled count once more here (a no-op duplicate of the last
        // in-loop report when the loop instead ran out its full sweep cap
        // without settling everything).
        control.report(SolveProgress {
            phase: SolvePhase::Constructive,
            sweep: last_constructive_sweep,
            max_sweeps: progress_count(MAX_CONSTRUCTIVE_SWEEPS),
            blocks_done: progress_count(settled.iter().filter(|&&s| s).count()),
            blocks_total: progress_count(n),
        });

        // ---- Phase 2: per-block estimates for everything still unsettled. ----
        if control.is_cancelled() {
            return Err(SolveError::Cancelled);
        }
        control.report(SolveProgress {
            phase: SolvePhase::LeastSquares,
            sweep: 1,
            max_sweeps: 1,
            blocks_done: 0,
            blocks_total: progress_count(n),
        });
        for i in 0..n {
            if settled[i] {
                continue;
            }
            let members: Vec<(f64, f64, f64)> = (0..n)
                .filter(|&j| settled[j] && blocks[j] == blocks[i])
                .map(|j| {
                    let y = normals[j][0].y.abs();
                    (y, y.mul_add(-y, 1.0).max(0.0).sqrt(), mast[j])
                })
                .collect();
            let y = normals[i][0].y.abs();
            mast[i] = block_estimate(&members, y, y.mul_add(-y, 1.0).max(0.0).sqrt(), scale_prior);
            origin[i] = Origin::Estimated;
        }

        // ---- Phase 3: nearest-level refinement sweeps over the full arrangement. ----
        let mut refine_sweeps = 0usize;
        let mut converged = false;
        for sweep_idx in 0..MAX_REFINE_SWEEPS {
            if control.is_cancelled() {
                return Err(SolveError::Cancelled);
            }
            control.report(SolveProgress {
                phase: SolvePhase::Refine,
                sweep: progress_count(sweep_idx + 1),
                max_sweeps: progress_count(MAX_REFINE_SWEEPS),
                blocks_done: 0,
                blocks_total: progress_count(n),
            });
            refine_sweeps += 1;
            // Re-enumerated from scratch each sweep; caching does not pay (see `candidates`).
            let mut planes = blank_planes();
            for (i, ns) in normals.iter().enumerate() {
                for &nv in ns {
                    planes.push(SolvePlane {
                        n: nv,
                        m: mast[i],
                        owner: i,
                    });
                }
            }
            // The dominant cost. The enumeration checks `control` per outer and middle
            // plane, so a cancel lands before the whole `O(P^3)` scan finishes.
            let Some(cands) = enumerate_candidate_vertices_cancellable(&planes, 6, control.cancel)
            else {
                return Err(SolveError::Cancelled);
            };
            let mut new_mast = mast.clone();

            for i in 0..n {
                if is_anchor[i] {
                    continue;
                }
                // Pre-sized: on large designs nearly every candidate is usable (~10^5 entries).
                let mut usable: Vec<&CandidateVertex> = Vec::with_capacity(cands.len());
                usable.extend(cands.iter().filter(|c| {
                    !c.owners.contains(&i) && (c.violated.is_none() || c.violated == Some(i))
                }));
                if usable.is_empty() {
                    continue;
                }
                let n0 = normals[i][0];
                let vals: Vec<f64> = usable.iter().map(|c| n0.dot(c.v)).collect();
                let levels = group_levels(vals.iter().copied().filter(|v| *v > 1e-9).collect());
                let levels = filter_levels_by_instance_support(levels, &normals[i], &usable);
                if levels.is_empty() {
                    continue;
                }

                // Prefer the shallowest level incident to every resolved named reference, as in
                // phase 1. Otherwise take the level nearest the current value.
                let refs = &resolved_named[i];
                // Index into `levels`, not the value -- see phase 1's named rule
                // comment for why.
                let nearest_of = || -> Option<usize> {
                    levels
                        .iter()
                        .enumerate()
                        .min_by(|&(_, a), &(_, b)| {
                            (a - mast[i])
                                .abs()
                                .partial_cmp(&(b - mast[i]).abs())
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(idx, _)| idx)
                };
                // All-or-nothing, as in phase 1. An overridden tier refines by nearest level
                // only, so the sweeps polish the forced pick instead of moving it back.
                let named_pool: Vec<usize> = if refs.is_empty() || overrides.contains_key(&i) {
                    Vec::new()
                } else {
                    levels
                        .iter()
                        .enumerate()
                        .filter(|&(_, &head)| {
                            usable.iter().zip(&vals).any(|(c, &val)| {
                                (val - head).abs() <= LEVEL_TOL
                                    && refs.iter().all(|&t| {
                                        normals[t]
                                            .iter()
                                            .any(|&nr| (nr.dot(c.v) - mast[t]).abs() < EPS_INCIDENT)
                                    })
                            })
                        })
                        .map(|(idx, _)| idx)
                        .collect()
                };
                let pick = named_pool.first().copied().or_else(nearest_of);
                if let Some(li) = pick {
                    let v = levels[li];
                    if v.is_finite() && v > 1e-9 && v <= domination_limit {
                        new_mast[i] = v;
                        last_pick[i] = Some((li, levels.len()));
                        if origin[i] == Origin::Estimated {
                            origin[i] = Origin::Refined;
                        }
                    }
                }
            }
            control.report(SolveProgress {
                phase: SolvePhase::Refine,
                sweep: progress_count(sweep_idx + 1),
                max_sweeps: progress_count(MAX_REFINE_SWEEPS),
                blocks_done: progress_count(n),
                blocks_total: progress_count(n),
            });

            // NaN-propagating fold: a plain `f64::max` would drop a NaN mast and could
            // declare false convergence. A NaN here makes the `< 1e-12` test below false.
            let max_rel_change = mast
                .iter()
                .zip(&new_mast)
                .map(|(&old, &new)| (new - old).abs() / old.abs().max(1e-9))
                .fold(0.0_f64, |acc, x| {
                    if acc.is_nan() || x.is_nan() {
                        f64::NAN
                    } else {
                        acc.max(x)
                    }
                });
            mast = new_mast;
            if max_rel_change < 1e-12 {
                converged = true;
                break;
            }
        }

        Ok(PipelineResult {
            mast,
            origin,
            last_pick,
            named_cause,
            refine_sweeps,
            converged,
        })
    }
}
