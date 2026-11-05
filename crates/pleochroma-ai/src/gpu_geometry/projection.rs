//! Adapted from cut-core optimize/constrained.rs. Structural preparation is once
//! per episode; finite differences and line-search residuals are batched on CUDA.
use super::{
    GatePhase, GpuGeometry, ProjectionMissReason, RequestCache,
    certificate::{Certificates, requested_shape_holds},
    network::{Network, Plane, Probe, scratch_per_probe},
    projection_step,
};
use pleochroma::geometry::meet_solver::{MeetConstraint, SolvedTier};
use pleochroma_cut_core::{
    Design,
    meet_preservation::{MeetPreservationBaseline, PreservationFacts, PreservationPolicy},
};
use rayon::prelude::*;
#[cfg(feature = "cuda")]
use std::sync::{OnceLock, atomic::AtomicBool};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::Ordering},
};

#[cfg(all(test, not(feature = "cuda")))]
#[path = "projection_cache_tests.rs"]
mod cache_tests;
#[path = "projection_search.rs"]
mod search;

pub(crate) struct Projection {
    backend: Arc<GpuGeometry>,
    network: Network,
    original: Design,
    free: Vec<usize>,
    masts: Vec<usize>,
    baseline: Arc<MeetPreservationBaseline>,
    policy: PreservationPolicy,
    certificates: Certificates,
    misses: RequestCache<ProjectionMissReason>,
    #[cfg(feature = "cuda")]
    device: OnceLock<Result<super::cuda::DeviceNetwork, String>>,
    // Rejection proofs use a CPU-captured baseline: production's clustering
    // and slack are discrete, and GPU capture parity cannot be inferred from
    // numerical closeness near its identify threshold.
    #[cfg(feature = "cuda")]
    proof_network: Option<Network>,
    #[cfg(feature = "cuda")]
    proof_device: OnceLock<Result<super::cuda::DeviceNetwork, String>>,
    #[cfg(feature = "cuda")]
    arrangement_refutations: super::certificate::ArrangementRefutations,
    /// In-budget arrangement proofs run for this episode, and how many refuted.
    #[cfg(feature = "cuda")]
    proof_runs: std::sync::atomic::AtomicU64,
    #[cfg(feature = "cuda")]
    proof_refuted: std::sync::atomic::AtomicU64,
    #[cfg(feature = "cuda")]
    parity: OnceLock<bool>,
    #[cfg(feature = "cuda")]
    failed: AtomicBool,
}

#[cfg(feature = "cuda")]
const ARRANGEMENT_REFUSAL: &str =
    "An original junction has no possible candidate arrangement point.";

/// Completed arrangement proofs an episode runs before judging whether they pay off.
#[cfg(feature = "cuda")]
const PROOF_TRIAL_RUNS: u64 = 64;

/// Proofs keep running only while at least one in this many refutes. A refutation
/// saves a ~50 ms CPU gate; a witnessed proof only adds its round trip.
#[cfg(feature = "cuda")]
const PROOF_MIN_REFUTATION_RATIO: u64 = 32;

#[derive(Clone, Copy)]
struct TrustRegion<'a> {
    origin: &'a [f64],
    limits: &'a [f64],
    bounds: &'a BTreeMap<usize, (f64, f64)>,
    legal: &'a [(f64, f64)],
}

#[cfg(feature = "cuda")]
fn same_plane_bits(left: &[Plane], right: &[Plane]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|((a, ad), (b, bd))| {
            a.x.to_bits() == b.x.to_bits()
                && a.y.to_bits() == b.y.to_bits()
                && a.z.to_bits() == b.z.to_bits()
                && ad.to_bits() == bd.to_bits()
        })
}

impl std::fmt::Debug for Projection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Projection")
            .field("junctions", &self.network.junctions.len())
            .field("variables", &(self.free.len() + self.masts.len()))
            .finish_non_exhaustive()
    }
}

impl Projection {
    pub(crate) fn prepare(
        original: &Design,
        free: &[usize],
        anchors: &BTreeSet<usize>,
        vary_anchored: bool,
        only_tiers: Option<&BTreeSet<usize>>,
        baseline: Arc<MeetPreservationBaseline>,
        policy: PreservationPolicy,
        backend: Arc<GpuGeometry>,
        original_solved: Option<&[SolvedTier]>,
    ) -> Option<Arc<Self>> {
        if !original.concave_tiers.is_empty() {
            backend.unsupported("concave authored tiers require the original geometry projection");
            return None;
        }
        let solid = backend.solid.as_deref();
        let prepared = match original_solved.map_or_else(
            || Network::capture_with_solid(original, solid),
            |solved| Network::capture_prepared(original, solid, solved),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                backend.fail(format!("CUDA preservation network capture failed: {error}"));
                return None;
            }
        };
        let Some(mut network) = prepared else {
            backend.unsupported(
                "canonical network preparation failed for the original authored solve",
            );
            return None;
        };
        network.width = baseline.width();
        network.half_length = 0.5 * baseline.shape().aspect * baseline.width();
        if scratch_per_probe(&network) > 4 * 1024 * 1024 {
            backend.unsupported("one geometry probe exceeds the 32 MiB CUDA scratch budget");
            return None;
        }
        let masts = (0..original.tiers.len())
            .filter(|&i| {
                vary_anchored
                    && original.tier_target(i).is_none()
                    && matches!(
                        original.tiers[i].constraint,
                        MeetConstraint::ScaleReference(_)
                    )
                    && (anchors.contains(&i) || original.tiers[i].angle_deg.abs() < 1e-8)
                    && only_tiers.is_none_or(|set| set.contains(&i))
                    && original.tiers[i].angle_deg.abs() < 89.999
            })
            .collect();
        backend.counters.prepared.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "cuda")]
        let proof_network = Network::capture_with_solid(original, None).ok().flatten();
        Some(Arc::new(Self {
            backend,
            network,
            original: original.clone(),
            free: free.to_vec(),
            masts,
            baseline,
            policy,
            certificates: Certificates::default(),
            misses: RequestCache::default(),
            #[cfg(feature = "cuda")]
            device: OnceLock::new(),
            #[cfg(feature = "cuda")]
            proof_network,
            #[cfg(feature = "cuda")]
            proof_device: OnceLock::new(),
            #[cfg(feature = "cuda")]
            arrangement_refutations: super::certificate::ArrangementRefutations::default(),
            #[cfg(feature = "cuda")]
            proof_runs: std::sync::atomic::AtomicU64::new(0),
            #[cfg(feature = "cuda")]
            proof_refuted: std::sync::atomic::AtomicU64::new(0),
            #[cfg(feature = "cuda")]
            parity: OnceLock::new(),
            #[cfg(feature = "cuda")]
            failed: AtomicBool::new(false),
        }))
    }

    fn batch(
        &self,
        candidates: &[Design],
        requested_travel: Option<f64>,
    ) -> Result<Vec<Probe>, ProjectionMissReason> {
        self.backend
            .check()
            .map_err(|_| ProjectionMissReason::ExecutionFailure)?;
        let authored_count = candidates
            .iter()
            .filter(|design| !super::network::direct_masts(design))
            .count();
        self.backend
            .counters
            .authoring
            .fetch_add(authored_count as u64, Ordering::Relaxed);
        // The authored solve remains exact and independent per probe. Nested Rayon
        // uses the existing rollout pool instead of creating another thread pool.
        let authoring_started = std::time::Instant::now();
        let valid: Vec<_> = if authored_count == 0 {
            candidates
                .iter()
                .enumerate()
                .filter_map(|(i, d)| Network::surfaces_for_probe(d).map(|p| (i, p)))
                .collect()
        } else {
            candidates
                .par_iter()
                .enumerate()
                .filter_map(|(i, d)| Network::surfaces_for_probe(d).map(|p| (i, p)))
                .collect()
        };
        super::elapsed(&self.backend.counters.authoring_ns, authoring_started);
        let (indices, planes): (Vec<_>, Vec<_>) = valid.into_iter().unzip();
        if planes.iter().any(|p| p.len() != self.network.surfaces) {
            return Err(ProjectionMissReason::PlaneProvenanceChanged);
        }
        let per = scratch_per_probe(&self.network);
        let capacity = (4 * 1024 * 1024 / per).clamp(1, 64);
        let mut result = vec![
            Probe {
                rows: None,
                possible: true
            };
            candidates.len()
        ];
        for (offset, chunk) in planes.chunks(capacity).enumerate() {
            let start = offset * capacity;
            let limits: Option<Vec<_>> = requested_travel.map(|request| {
                indices[start..start + chunk.len()]
                    .iter()
                    .map(|&i| {
                        self.effective_policy(&candidates[i], request)
                            .max_junction_displacement
                            * self.network.scale
                    })
                    .collect()
            });
            // A residual's fast pinned-mast plane path is only an optimization;
            // reject candidates only when it exactly matches production planes.
            // Authored (non-direct) probes already used the exact solve itself
            // (`surfaces_for_probe`), so solving them again would only repeat it.
            #[cfg(feature = "cuda")]
            let limits = if limits.is_some()
                && indices[start..start + chunk.len()]
                    .iter()
                    .zip(chunk)
                    .any(|(&i, fast)| {
                        super::network::direct_masts(&candidates[i])
                            && super::network::surfaces(&candidates[i])
                                .is_none_or(|exact| !same_plane_bits(&exact, fast))
                    }) {
                None
            } else {
                limits
            };
            for (&i, row) in indices[offset * capacity..].iter().zip(
                self.residual_chunk(chunk, limits.as_deref())
                    .ok_or(ProjectionMissReason::ExecutionFailure)?,
            ) {
                result[i] = row;
            }
        }
        Ok(result)
    }

    fn residual_chunk(&self, planes: &[Vec<Plane>], limits: Option<&[f64]>) -> Option<Vec<Probe>> {
        #[cfg(feature = "cuda")]
        if !self.failed.load(Ordering::Relaxed) {
            // Residuals can use the GPU-captured network independently. A
            // rejection proof additionally requires exact CPU baseline parity;
            // clustering and slack can change discontinuously at a boundary.
            let limits = limits.filter(|_| {
                self.proof_network
                    .as_ref()
                    .is_some_and(|cpu| self.network.same_proof_metadata(cpu))
            });
            let device = self
                .device
                .get_or_init(|| self.backend.runtime.prepare(&self.network));
            let residual_started = std::time::Instant::now();
            let result = match device {
                Ok(device) => self.backend.runtime.batch(
                    &self.network,
                    device,
                    planes,
                    limits,
                    self.policy.fixed_outline,
                    self.policy.lock_girdle_gauge,
                ),
                Err(error) => Err(error.clone()),
            };
            super::elapsed(&self.backend.counters.residual_ns, residual_started);
            let failure = match result {
                Ok(rows) => {
                    self.backend
                        .counters
                        .batches
                        .fetch_add(1, Ordering::Relaxed);
                    self.backend
                        .counters
                        .probes
                        .fetch_add(rows.len() as u64, Ordering::Relaxed);
                    // First batch checks each row of its first candidate against an
                    // independent f64 CPU reference. Absolute contract: 2e-12*(1+|r|),
                    // one millionth of the preservation tolerance near the solution.
                    let parity = self.parity.get().copied().unwrap_or_else(|| {
                        let reference = planes.iter().zip(&rows).find_map(|(p, probe)| {
                            self.network
                                .residuals(p)
                                .map(|cpu| (probe.rows.as_ref(), cpu))
                        });
                        let Some((row, cpu)) = reference else {
                            return true;
                        };
                        *self.parity.get_or_init(|| {
                            row.is_some_and(|row| {
                                row.len() == cpu.len()
                                    && row.iter().zip(cpu).all(|(a, b)| {
                                        a.is_finite() && (a - b).abs() <= 2e-12 * (1.0 + b.abs())
                                    })
                            })
                        })
                    });
                    if parity {
                        // A singular basis is an invalid geometry probe, which the
                        // CPU reference also returns as None. It is not a GPU fault.
                        return Some(rows);
                    }
                    "CUDA geometry residuals failed the f64 CPU parity/finite contract".to_string()
                }
                Err(error) => error,
            };
            self.failed.store(true, Ordering::Relaxed);
            self.backend.fail(failure);
            return None;
        }
        #[cfg(feature = "cuda")]
        {
            let _ = (planes, limits);
            None
        }
        #[cfg(not(feature = "cuda"))]
        {
            self.backend
                .counters
                .cpu_batches
                .fetch_add(1, Ordering::Relaxed);
            let _ = limits;
            Some(
                planes
                    .iter()
                    .map(|p| Probe {
                        rows: self.network.residuals(p),
                        possible: true,
                    })
                    .collect(),
            )
        }
    }

    fn materialize(
        &self,
        base: &Design,
        values: &[f64],
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Option<Design> {
        if values.len() != self.free.len() + self.masts.len() {
            return None;
        }
        let mut candidate = base.clone();
        for (&i, &angle) in self.free.iter().zip(values) {
            let old = self.original.tiers[i].angle_deg;
            if !angle.is_finite()
                || angle == 0.0
                || angle.abs() > pleochroma_cut_core::optimize::MAX_SAFE_CANDIDATE_ANGLE_DEG
                || old.is_sign_negative() != angle.is_sign_negative()
                || bounds.get(&i).is_some_and(|&(lo, hi)| {
                    !lo.is_finite() || !hi.is_finite() || lo > hi || angle < lo || angle > hi
                })
            {
                return None;
            }
            candidate.tiers[i].angle_deg = angle;
        }
        for (&i, &mast) in self.masts.iter().zip(&values[self.free.len()..]) {
            if !mast.is_finite() || mast <= 1e-9 {
                return None;
            }
            if let MeetConstraint::ScaleReference(current) = candidate.tiers[i].constraint
                && (current / self.network.scale).to_bits() == mast.to_bits()
            {
                continue;
            }
            candidate.tiers[i].constraint =
                MeetConstraint::ScaleReference(mast * self.network.scale);
        }
        for (i, angle) in candidate.evaluate_relations().ok()? {
            let tier = candidate.tiers.get_mut(i)?;
            if tier.angle_deg.to_bits() == angle.to_bits() {
                continue;
            }
            if !angle.is_finite()
                || angle == 0.0
                || angle.abs() > pleochroma_cut_core::optimize::MAX_SAFE_CANDIDATE_ANGLE_DEG
                || tier.angle_deg.is_sign_negative() != angle.is_sign_negative()
            {
                return None;
            }
            tier.angle_deg = angle;
        }
        Some(candidate)
    }

    fn effective_policy(&self, candidate: &Design, requested_travel: f64) -> PreservationPolicy {
        // Match core assess(Some(requested_turn)): a repair's own turn cannot buy
        // more travel than the seed requested. Freeze that exact allowance before
        // using the public gate so its default own-turn allowance cannot widen it.
        let mut policy = self.policy;
        if policy.bounds_follow_turn {
            let travel = requested_travel.min(self.network.turn_travel(&self.original, candidate));
            policy = policy.for_turn(travel / self.network.width, travel / self.network.scale);
            policy.bounds_follow_turn = false;
        }
        policy
    }

    fn original_facts(
        &self,
        candidate: &Design,
    ) -> Result<PreservationFacts, (ProjectionMissReason, String)> {
        self.original_facts_at(candidate, GatePhase::ExplicitCertification)
    }

    fn original_facts_at(
        &self,
        candidate: &Design,
        phase: GatePhase,
    ) -> Result<PreservationFacts, (ProjectionMissReason, String)> {
        self.backend
            .check()
            .map_err(|error| (ProjectionMissReason::ExecutionFailure, error))?;
        // One authoritative original-policy gate supplies both the certificate and
        // the exact authored solve consumed by subsequent optical evaluation.
        if let Some(facts) = self.certified_facts(candidate) {
            return Ok(facts);
        }
        if let Some(error) = self.certificates.rejection(candidate) {
            self.backend
                .counters
                .negative_hits
                .fetch_add(1, Ordering::Relaxed);
            return Err((ProjectionMissReason::OriginalGateRejected, error));
        }
        #[cfg(feature = "cuda")]
        {
            let started = std::time::Instant::now();
            // A completed refutation depends on exact ordered production planes,
            // not the authored f64 state that may sit on the same f32 plateau.
            let over_budget = self
                .proof_network
                .as_ref()
                .is_some_and(|network| network.arrangement_triples_within_budget().is_none());
            // An episode whose proofs keep witnessing stops proving: each proof is a
            // synchronous CUDA round trip in front of a CPU gate that then runs anyway.
            // Its completed refutations stay in the cache below only while proving.
            let unproductive = !over_budget && self.proofs_unproductive();
            if unproductive {
                self.backend
                    .counters
                    .arrangement_unproductive
                    .fetch_add(1, Ordering::Relaxed);
            }
            // This immutable network cannot have completed proof-cache entries
            // when over budget; skip the extra authored solve/plane packing too.
            let planes = self
                .proof_network
                .as_ref()
                .filter(|_| !over_budget && !unproductive)
                .and_then(|_| super::network::surfaces(candidate));
            if let Some(planes) = &planes
                && self.arrangement_refutations.contains(planes)
            {
                self.certificates
                    .insert_rejection(candidate, ARRANGEMENT_REFUSAL.into());
                self.backend
                    .counters
                    .negative_hits
                    .fetch_add(1, Ordering::Relaxed);
                super::elapsed(&self.backend.counters.arrangement_ns, started);
                return Err((
                    ProjectionMissReason::OriginalGateRejected,
                    ARRANGEMENT_REFUSAL.into(),
                ));
            }
            self.backend
                .counters
                .arrangement_probes
                .fetch_add(1, Ordering::Relaxed);
            let mut work = super::cuda::ArrangementWork::default();
            // Use the production schedule path, not the faster residual-only
            // approximation, because this result can avoid an authoritative gate.
            let proof = if over_budget || unproductive {
                Ok(super::cuda::ArrangementOutcome::Capacity)
            } else if let (Some(network), Some(planes)) = (&self.proof_network, planes.as_deref()) {
                // Only an in-budget search prepares immutable proof buffers.
                let device = self
                    .proof_device
                    .get_or_init(|| self.backend.runtime.prepare(network));
                match device {
                    Ok(device) => self.backend.runtime.arrangement_proof(
                        network,
                        device,
                        planes,
                        &mut work,
                        &|| self.backend.check(),
                    ),
                    Err(error) => Err(error.clone()),
                }
            } else {
                Ok(super::cuda::ArrangementOutcome::Uncertain)
            };
            super::elapsed(&self.backend.counters.arrangement_ns, started);
            self.backend
                .counters
                .arrangement_tiles
                .fetch_add(work.tiles, Ordering::Relaxed);
            self.backend
                .counters
                .arrangement_triples
                .fetch_add(work.triples, Ordering::Relaxed);
            if work.tiles > 0 {
                self.proof_runs.fetch_add(1, Ordering::Relaxed);
            }
            match proof {
                Ok(super::cuda::ArrangementOutcome::Refuted) => {
                    self.proof_refuted.fetch_add(1, Ordering::Relaxed);
                    if let Some(planes) = &planes {
                        self.arrangement_refutations.insert_refuted(planes);
                    }
                    self.certificates
                        .insert_rejection(candidate, ARRANGEMENT_REFUSAL.into());
                    self.backend
                        .counters
                        .arrangement_refusals
                        .fetch_add(1, Ordering::Relaxed);
                    return Err((
                        ProjectionMissReason::OriginalGateRejected,
                        ARRANGEMENT_REFUSAL.into(),
                    ));
                }
                Ok(super::cuda::ArrangementOutcome::Uncertain) => {
                    self.backend
                        .counters
                        .arrangement_uncertain
                        .fetch_add(1, Ordering::Relaxed);
                }
                Ok(super::cuda::ArrangementOutcome::Capacity) => {
                    if !unproductive {
                        self.backend
                            .counters
                            .arrangement_capacity_bypasses
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(super::cuda::ArrangementOutcome::Witnessed) => {
                    self.backend
                        .counters
                        .arrangement_witnessed
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    self.backend.fail(error.clone());
                    return Err((ProjectionMissReason::ExecutionFailure, error));
                }
            }
        }
        let started = std::time::Instant::now();
        let result = self
            .baseline
            .validate_and_solve_with(candidate, self.policy, None);
        self.backend
            .record_cpu_gate(started, phase, result.as_ref().err());
        match result {
            Ok((facts, solved)) => {
                self.certificates.insert(candidate, facts, solved);
                Ok(facts)
            }
            Err(error) => {
                let kind = error.kind();
                let reason = if super::cacheable_violation(kind) {
                    self.certificates
                        .insert_rejection(candidate, error.to_string());
                    ProjectionMissReason::OriginalGateRejected
                } else if kind == "cancelled" {
                    ProjectionMissReason::Cancelled
                } else {
                    ProjectionMissReason::UnclassifiedGateFailure
                };
                Err((reason, error.to_string()))
            }
        }
    }

    /// Whether this episode's arrangement proofs have earned their round trips: after
    /// [`PROOF_TRIAL_RUNS`] completed proofs, at least one in
    /// [`PROOF_MIN_REFUTATION_RATIO`] must have refuted. Skipping a proof never
    /// changes an outcome; the CPU gate then decides alone, as for large networks.
    #[cfg(feature = "cuda")]
    fn proofs_unproductive(&self) -> bool {
        let runs = self.proof_runs.load(Ordering::Relaxed);
        runs >= PROOF_TRIAL_RUNS
            && self
                .proof_refuted
                .load(Ordering::Relaxed)
                .saturating_mul(PROOF_MIN_REFUTATION_RATIO)
                < runs
    }

    #[cfg(test)]
    fn valid(&self, candidate: &Design, requested_travel: f64) -> Result<(), ProjectionMissReason> {
        self.valid_at(candidate, requested_travel, GatePhase::FinalIterate)
    }

    fn valid_at(
        &self,
        candidate: &Design,
        requested_travel: f64,
        phase: GatePhase,
    ) -> Result<(), ProjectionMissReason> {
        let result = self
            .original_facts_at(candidate, phase)
            .map_err(|(reason, _)| reason)
            .and_then(|facts| {
                if requested_shape_holds(
                    self.baseline.shape(),
                    facts,
                    self.effective_policy(candidate, requested_travel),
                ) {
                    Ok(())
                } else {
                    self.backend.record_requested_limit(phase);
                    Err(ProjectionMissReason::RequestedLimitsRejected)
                }
            });
        if result.is_err() {
            self.backend
                .counters
                .refusals
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    pub(crate) fn certify(&self, candidate: &Design) -> Result<PreservationFacts, String> {
        self.original_facts(candidate).map_err(|(_, error)| error)
    }

    pub(crate) fn certified_facts(&self, design: &Design) -> Option<PreservationFacts> {
        let result = self.certificates.facts(design);
        if result.is_some() {
            self.backend
                .counters
                .cache_hits
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    pub(crate) fn certified_solve(&self, design: &Design) -> Option<Vec<SolvedTier>> {
        let result = self.certificates.solved(design);
        if result.is_some() {
            self.backend
                .counters
                .cache_hits
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Return only a fully certified state, or refuse this accelerated correction.
    #[cfg(test)]
    pub(crate) fn project(
        &self,
        proposal: &Design,
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Option<Design> {
        self.project_classified(proposal, bounds).ok()
    }

    pub(crate) fn project_classified(
        &self,
        proposal: &Design,
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Result<Design, ProjectionMissReason> {
        let started = std::time::Instant::now();
        self.backend
            .counters
            .projections
            .fetch_add(1, Ordering::Relaxed);
        let result = if self.backend.check().is_err() {
            Err(ProjectionMissReason::ExecutionFailure)
        } else if let Some(reason) = self.misses.get(proposal, bounds) {
            self.backend
                .counters
                .projection_miss_hits
                .fetch_add(1, Ordering::Relaxed);
            Err(reason)
        } else {
            let result = self.project_inner(proposal, bounds);
            if self.backend.check().is_err() {
                Err(ProjectionMissReason::ExecutionFailure)
            } else {
                if let Err(reason) = result.as_ref()
                    && reason.cacheable()
                {
                    self.misses.insert(proposal, bounds, *reason);
                }
                result
            }
        };
        super::elapsed(&self.backend.counters.projection_ns, started);
        if let Err(reason) = &result {
            self.backend.record_miss(*reason);
        }
        result
    }

    fn project_inner(
        &self,
        proposal: &Design,
        bounds: &BTreeMap<usize, (f64, f64)>,
    ) -> Result<Design, ProjectionMissReason> {
        self.backend
            .check()
            .map_err(|_| ProjectionMissReason::ExecutionFailure)?;
        if proposal.tiers.len() != self.original.tiers.len() {
            return Err(ProjectionMissReason::InvalidSeed);
        }
        let mut seed = proposal.clone();
        for (i, tier) in seed.tiers.iter_mut().enumerate() {
            let original = &self.original.tiers[i];
            if !self.free.contains(&i)
                && !seed.tier_relations.contains_key(
                    &self
                        .original
                        .tier_id_at(i)
                        .unwrap_or(pleochroma_cut_core::TierId(i as u64)),
                )
            {
                tier.angle_deg = original.angle_deg;
            }
            if !self.masts.contains(&i) {
                tier.constraint = original.constraint.clone();
            }
        }
        let mut origin: Vec<_> = self.free.iter().map(|&i| seed.tiers[i].angle_deg).collect();
        for &i in &self.masts {
            let MeetConstraint::ScaleReference(mast) = seed.tiers[i].constraint else {
                return Err(ProjectionMissReason::InvalidSeed);
            };
            origin.push(mast / self.network.scale);
        }
        let seed = self
            .materialize(&seed, &origin, bounds)
            .ok_or(ProjectionMissReason::InvalidSeed)?;
        let requested_travel = self.network.turn_travel(&self.original, &seed);
        if self.certificates.facts(&seed).is_some() {
            match self.valid_at(&seed, requested_travel, GatePhase::Seed) {
                Ok(()) => return Ok(seed),
                Err(
                    ProjectionMissReason::OriginalGateRejected
                    | ProjectionMissReason::RequestedLimitsRejected,
                ) => {}
                Err(reason) => return Err(reason),
            }
        }
        let mut first_batch = self.batch(std::slice::from_ref(&seed), Some(requested_travel))?;
        let first = first_batch
            .pop()
            .ok_or(ProjectionMissReason::UnusableSeedResidual)?;
        // A seed whose rows already rule out acceptance goes straight to correction.
        let rejection = if first
            .rows
            .as_deref()
            .is_some_and(|rows| !self.may_pass_gate(rows))
        {
            self.backend.screened_gate();
            None
        } else {
            match self.gate_probe(&seed, requested_travel, &first, GatePhase::Seed)? {
                GateProbe::Accepted => return Ok(seed),
                GateProbe::Rejected(reason) => Some(reason),
                GateProbe::Impossible => None,
            }
        };
        if first.rows.is_none() {
            return Err(ProjectionMissReason::UnusableSeedResidual);
        }
        self.correct(&seed, &origin, first, rejection, requested_travel, bounds)
    }

    fn gate_probe(
        &self,
        candidate: &Design,
        requested_travel: f64,
        probe: &Probe,
        phase: GatePhase,
    ) -> Result<GateProbe, ProjectionMissReason> {
        if !probe.possible {
            self.backend
                .counters
                .prefilter
                .fetch_add(1, Ordering::Relaxed);
            return Ok(GateProbe::Impossible);
        }
        match self.valid_at(candidate, requested_travel, phase) {
            Ok(()) => Ok(GateProbe::Accepted),
            Err(
                reason @ (ProjectionMissReason::OriginalGateRejected
                | ProjectionMissReason::RequestedLimitsRejected),
            ) => Ok(GateProbe::Rejected(reason)),
            Err(reason) => Err(reason),
        }
    }
}

#[derive(Clone, Copy)]
enum GateProbe {
    Accepted,
    Rejected(ProjectionMissReason),
    Impossible,
}
