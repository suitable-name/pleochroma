//! Cached meet projection with native CUDA f64 residual batches.
//!
//! The GPU is a numeric correction engine. The immutable cut-core preservation
//! baseline remains the sole authority for every returned candidate. Pinned-mast
//! authoring (including angular relations) uses the exact production plane path;
//! named meets and dimensional targets keep the exact authored CPU solve before
//! CUDA incidence evaluation. Concave tiers use the counted CPU fallback.
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

mod certificate;
#[cfg(feature = "cuda")]
mod cuda;
mod network;
#[cfg(all(test, feature = "cuda"))]
mod prefilter_tests;
mod projection;
mod projection_diagnostics;
mod projection_step;
#[cfg(all(test, feature = "cuda"))]
mod projection_tests;
mod reach;
mod request_cache;
#[cfg(test)]
mod tests;
pub(crate) use certificate::same_state as same_design_state;
pub(crate) use projection::Projection;
pub use projection_diagnostics::ProjectionFallbackDiagnostics;
pub use projection_diagnostics::ProjectionGateDiagnostics;
pub(crate) use projection_diagnostics::ProjectionMissReason;
pub(crate) use projection_diagnostics::cacheable_violation;
pub(crate) use projection_diagnostics::{GatePhase, diagnostic_reason};
pub(crate) use request_cache::RequestCache;

/// Per-run geometry counters. CPU validation is required even for CUDA projections.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GeometryDiagnostics {
    /// Immutable networks prepared for supported episodes.
    pub prepared_networks: u64,
    /// Episode authoring forms that require the original CPU projection.
    pub unsupported_networks: u64,
    /// Batched CUDA residual launches, each containing many junction solves.
    pub cuda_batches: u64,
    /// Candidate residual vectors evaluated on CUDA (finite differences included).
    pub cuda_probes: u64,
    /// Residual batches evaluated by the CPU reference (never retried after a CUDA failure).
    pub cpu_residual_batches: u64,
    /// Projections sent to the original CPU implementation.
    pub cpu_projection_fallbacks: u64,
    /// Cached projection candidates rejected by the original gate or request limits.
    pub gate_refusals: u64,
    /// Numerical or driver failures on the CUDA correction path.
    pub cuda_failures: u64,
    /// Last CUDA driver, compiler or numerical-contract failure.
    pub last_failure: Option<String>,
    /// Candidate planes requiring the exact CPU authoring solve before CUDA rows.
    pub cpu_authoring_probes: u64,
    /// Explicit structural/preparation fallback reasons and episode counts.
    pub unsupported_reasons: BTreeMap<String, u64>,
    /// Full production gates avoided by outward-rounded impossible-meet proofs.
    pub gpu_prefilter_refusals: u64,
    /// Completed CPU gates called by the cached projection or explicit certification.
    /// The original fallback projection's internal gates remain in baseline telemetry.
    pub cpu_gate_calls: u64,
    /// Wall time inside those mandatory CPU gates; concurrent calls may overlap.
    pub cpu_gate_ms: f64,
    /// Exact complete-state certificate reads that avoided a gate or authored solve.
    pub certificate_cache_hits: u64,
    /// Exact authored CPU solve/plane-preparation wall time, summed across batches.
    pub cpu_authoring_service_ms: f64,
    /// Small coupled damped-QR correction wall time.
    pub cpu_qr_ms: f64,
    /// Accelerated host projection requests, including CPU work and CUDA waiting.
    pub projection_requests: u64,
    /// Sum of accelerated projection service times; excludes subsequent CPU fallback.
    /// Nested authoring, QR, residual and gate timers must not be added to this total.
    pub projection_service_ms: f64,
    /// Packing, transfers, launches and readback wall time; includes CUDA waiting.
    pub cuda_residual_service_ms: f64,
    /// Service time spent inside the explicit original CPU projection fallback.
    pub cpu_projection_fallback_ms: f64,
    /// Accelerated requests that stopped without a certificate, classified by cause.
    pub projection_miss_reasons: BTreeMap<String, u64>,
    /// Original CPU projection outcomes/service time, classified by accelerated miss.
    pub cpu_fallback_by_reason: BTreeMap<String, ProjectionFallbackDiagnostics>,
    /// Actual original-policy CPU gates that rejected the complete authored state.
    pub original_gate_refusals: u64,
    /// Original-policy certified candidates rejected by stricter request limits.
    pub requested_limit_refusals: u64,
    /// Exact negative original-policy gate cache reads, without a CPU gate call.
    pub negative_gate_cache_hits: u64,
    /// Exact repeated unsuccessful accelerated requests, without another correction.
    pub projection_miss_cache_hits: u64,
    /// Actual accelerated/explicit CPU gate refusals by stable core violation kind.
    pub original_gate_rejection_kinds: BTreeMap<String, u64>,
    /// At most four first refusal examples, each limited to 384 characters.
    pub original_gate_rejection_examples: BTreeMap<String, String>,
    /// Errors returned by original CPU fallback calls, excluding cache reads.
    pub fallback_rejection_kinds: BTreeMap<String, u64>,
    /// At most four first fallback refusal examples, each limited to 384 characters.
    pub fallback_rejection_examples: BTreeMap<String, String>,
    /// Stages reported by failed original CPU projection calls.
    pub fallback_failure_stages: BTreeMap<String, u64>,
    /// Completed CPU gate counts/times separated into seed, trial, final and certification.
    pub gate_phases: BTreeMap<String, ProjectionGateDiagnostics>,
}

#[derive(Default)]
struct Counters {
    prepared: AtomicU64,
    unsupported: AtomicU64,
    batches: AtomicU64,
    probes: AtomicU64,
    cpu_batches: AtomicU64,
    fallbacks: AtomicU64,
    refusals: AtomicU64,
    failures: AtomicU64,
    authoring: AtomicU64,
    prefilter: AtomicU64,
    gates: AtomicU64,
    gate_ns: AtomicU64,
    cache_hits: AtomicU64,
    authoring_ns: AtomicU64,
    qr_ns: AtomicU64,
    projection_ns: AtomicU64,
    projections: AtomicU64,
    residual_ns: AtomicU64,
    fallback_ns: AtomicU64,
    original_refusals: AtomicU64,
    requested_refusals: AtomicU64,
    negative_hits: AtomicU64,
    projection_miss_hits: AtomicU64,
    classified: Mutex<projection_diagnostics::ClassifiedCounts>,
}

/// Native CUDA geometry service shared only by environments of the owning run.
/// It uses the selected primary CUDA context and never initializes WGPU.
pub struct GpuGeometry {
    #[cfg(feature = "cuda")]
    runtime: cuda::Runtime,
    counters: Counters,
    failure: Mutex<Option<String>>,
    unsupported: Mutex<BTreeMap<String, u64>>,
    solid: Option<Arc<crate::gpu_solid::GpuSolid>>,
}

impl std::fmt::Debug for GpuGeometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuGeometry")
            .field("diagnostics", &self.diagnostics())
            .finish_non_exhaustive()
    }
}

impl GpuGeometry {
    /// Compile the f64 residual kernels on the same device ordinal as Burn.
    ///
    /// # Errors
    /// Returns the CUDA/NVRTC initialization error, or a missing-feature error.
    pub fn new_cuda(ordinal: usize) -> Result<Arc<Self>, String> {
        let solid = crate::gpu_solid::GpuSolid::new_cuda(ordinal)?;
        Self::new_cuda_with_solid(ordinal, solid)
    }

    /// Compile projection kernels while sharing the run's existing solid service.
    ///
    /// # Errors
    /// Returns solid-service, CUDA or NVRTC initialization errors.
    pub fn new_cuda_with_solid(
        ordinal: usize,
        solid: Arc<crate::gpu_solid::GpuSolid>,
    ) -> Result<Arc<Self>, String> {
        #[cfg(feature = "cuda")]
        {
            solid.check()?;
            Ok(Arc::new(Self {
                runtime: cuda::Runtime::new(ordinal)?,
                counters: Counters::default(),
                failure: Mutex::new(None),
                unsupported: Mutex::new(BTreeMap::new()),
                solid: Some(solid),
            }))
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = (ordinal, solid);
            Err("CUDA geometry requires the pleochroma-ai cuda feature".into())
        }
    }

    /// Read cumulative counters for the owning run, including all fallbacks.
    #[must_use]
    pub fn diagnostics(&self) -> GeometryDiagnostics {
        let c = &self.counters;
        let classified = c
            .classified
            .lock()
            .map(|counts| counts.clone())
            .unwrap_or_default();
        GeometryDiagnostics {
            prepared_networks: c.prepared.load(Ordering::Relaxed),
            unsupported_networks: c.unsupported.load(Ordering::Relaxed),
            cuda_batches: c.batches.load(Ordering::Relaxed),
            cuda_probes: c.probes.load(Ordering::Relaxed),
            cpu_residual_batches: c.cpu_batches.load(Ordering::Relaxed),
            cpu_projection_fallbacks: c.fallbacks.load(Ordering::Relaxed),
            gate_refusals: c.refusals.load(Ordering::Relaxed),
            cuda_failures: c.failures.load(Ordering::Relaxed),
            last_failure: self.failure.lock().ok().and_then(|failure| failure.clone()),
            cpu_authoring_probes: c.authoring.load(Ordering::Relaxed),
            unsupported_reasons: self
                .unsupported
                .lock()
                .map(|reasons| reasons.clone())
                .unwrap_or_default(),
            gpu_prefilter_refusals: c.prefilter.load(Ordering::Relaxed),
            cpu_gate_calls: c.gates.load(Ordering::Relaxed),
            cpu_gate_ms: c.gate_ns.load(Ordering::Relaxed) as f64 / 1e6,
            certificate_cache_hits: c.cache_hits.load(Ordering::Relaxed),
            cpu_authoring_service_ms: c.authoring_ns.load(Ordering::Relaxed) as f64 / 1e6,
            cpu_qr_ms: c.qr_ns.load(Ordering::Relaxed) as f64 / 1e6,
            projection_requests: c.projections.load(Ordering::Relaxed),
            projection_service_ms: c.projection_ns.load(Ordering::Relaxed) as f64 / 1e6,
            cuda_residual_service_ms: c.residual_ns.load(Ordering::Relaxed) as f64 / 1e6,
            cpu_projection_fallback_ms: c.fallback_ns.load(Ordering::Relaxed) as f64 / 1e6,
            projection_miss_reasons: classified.misses,
            cpu_fallback_by_reason: classified.fallbacks,
            original_gate_refusals: c.original_refusals.load(Ordering::Relaxed),
            requested_limit_refusals: c.requested_refusals.load(Ordering::Relaxed),
            negative_gate_cache_hits: c.negative_hits.load(Ordering::Relaxed),
            projection_miss_cache_hits: c.projection_miss_hits.load(Ordering::Relaxed),
            original_gate_rejection_kinds: classified.original_gate_rejection_kinds,
            original_gate_rejection_examples: classified.original_gate_rejection_examples,
            fallback_rejection_kinds: classified.fallback_rejection_kinds,
            fallback_rejection_examples: classified.fallback_rejection_examples,
            fallback_failure_stages: classified.fallback_failure_stages,
            gate_phases: classified.gate_phases,
        }
    }

    pub(crate) fn fallback(&self, reason: ProjectionMissReason) {
        self.counters.fallbacks.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut counts) = self.counters.classified.lock() {
            counts.fallback(reason).calls += 1;
        }
    }

    pub(crate) fn record_cpu_gate(
        &self,
        started: std::time::Instant,
        phase: GatePhase,
        error: Option<&pleochroma_cut_core::meet_preservation::PreservationViolation>,
    ) {
        let ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        self.counters.gates.fetch_add(1, Ordering::Relaxed);
        self.counters.gate_ns.fetch_add(ns, Ordering::Relaxed);
        if error.is_some() {
            self.counters
                .original_refusals
                .fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut counts) = self.counters.classified.lock() {
            counts.gate(phase, ns as f64 / 1e6, error);
        }
    }

    fn record_requested_limit(&self, phase: GatePhase) {
        self.counters
            .requested_refusals
            .fetch_add(1, Ordering::Relaxed);
        if let Ok(mut counts) = self.counters.classified.lock() {
            counts.requested_limit(phase);
        }
    }

    pub(crate) fn record_fallback(
        &self,
        started: std::time::Instant,
        reason: ProjectionMissReason,
        error: Option<&pleochroma_cut_core::meet_preservation::PreservationViolation>,
    ) {
        let ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        self.counters.fallback_ns.fetch_add(ns, Ordering::Relaxed);
        if let Ok(mut counts) = self.counters.classified.lock() {
            let outcome = counts.fallback(reason);
            outcome.service_ms += ns as f64 / 1e6;
            if error.is_none() {
                outcome.succeeded += 1;
                outcome.succeeded_service_ms += ns as f64 / 1e6;
            } else {
                outcome.refused += 1;
                outcome.refused_service_ms += ns as f64 / 1e6;
            }
            if let Some(error) = error {
                counts.fallback_refusal(error);
            }
        }
    }

    pub(crate) fn record_fallback_cache(&self, reason: ProjectionMissReason, succeeded: bool) {
        if let Ok(mut counts) = self.counters.classified.lock() {
            let outcome = counts.fallback(reason);
            if succeeded {
                outcome.cached_successes += 1;
            } else {
                outcome.cached_refusals += 1;
            }
        }
    }

    fn record_miss(&self, reason: ProjectionMissReason) {
        if let Ok(mut counts) = self.counters.classified.lock() {
            counts.miss(reason);
        }
    }

    fn unsupported(&self, reason: &str) {
        self.counters.unsupported.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut reasons) = self.unsupported.lock() {
            *reasons.entry(reason.to_string()).or_default() += 1;
        }
    }

    /// Propagate a latched runtime error to the owning training/evaluation run.
    ///
    /// # Errors
    /// Returns the first CUDA driver or numerical-contract failure. Unsupported
    /// authoring is counted separately and does not fail this check.
    pub fn check(&self) -> Result<(), String> {
        if let Some(solid) = &self.solid {
            solid.check()?;
        }
        self.failure
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map_or(Ok(()), |error| Err(error.clone()))
    }

    fn fail(&self, reason: String) {
        self.counters.failures.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut failure) = self.failure.lock()
            && failure.is_none()
        {
            *failure = Some(reason);
        }
    }
}

fn elapsed(counter: &AtomicU64, started: std::time::Instant) {
    counter.fetch_add(
        started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
        Ordering::Relaxed,
    );
}
