//! Classified host projection work. Timings are summed service time, not utilization.
use pleochroma_cut_core::meet_preservation::PreservationViolation;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub(crate) enum GatePhase {
    Seed,
    LineTrial,
    FinalIterate,
    ExplicitCertification,
}

impl GatePhase {
    fn label(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::LineTrial => "line_trial",
            Self::FinalIterate => "final_iterate",
            Self::ExplicitCertification => "explicit_certification",
        }
    }
}

/// Completed original CPU gate work by caller phase; cached reads are excluded.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProjectionGateDiagnostics {
    /// Actual completed calls to the original gate.
    pub cpu_calls: u64,
    /// Calls whose original gate returned a violation.
    pub refused: u64,
    /// Summed wall service time inside those calls.
    pub service_ms: f64,
    /// Certified facts that failed the request's stricter limits, including cache reads.
    pub requested_limit_refusals: u64,
}

/// Short diagnostic text without authored surface-name lists or complete designs.
pub(crate) fn diagnostic_reason(error: &PreservationViolation) -> String {
    let text = match error {
        PreservationViolation::AtSurfaces { detail, .. } => detail.clone(),
        _ => error.to_string(),
    };
    text.split_once(" Affected surfaces:")
        .map_or(text.as_str(), |(reason, _)| reason)
        .chars()
        .take(384)
        .collect()
}

fn refusal(
    kinds: &mut BTreeMap<String, u64>,
    examples: &mut BTreeMap<String, String>,
    error: &PreservationViolation,
) {
    let kind = error.kind();
    *kinds.entry(kind.into()).or_default() += 1;
    if examples.len() < 4 && !examples.contains_key(kind) {
        examples.insert(kind.into(), diagnostic_reason(error));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProjectionMissReason {
    UnsupportedNetwork,
    InvalidSeed,
    PlaneProvenanceChanged,
    UnusableSeedResidual,
    InvalidLinearization,
    InvalidCorrection,
    NoFeasibleDirection,
    NoFeasibleCandidate,
    NoResidualProgress,
    OriginalGateRejected,
    RequestedLimitsRejected,
    ExecutionFailure,
    Cancelled,
    UnclassifiedGateFailure,
}

impl ProjectionMissReason {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::UnsupportedNetwork => "unsupported_network",
            Self::InvalidSeed => "invalid_seed",
            Self::PlaneProvenanceChanged => "plane_provenance_changed",
            Self::UnusableSeedResidual => "unusable_seed_residual",
            Self::InvalidLinearization => "invalid_linearization",
            Self::InvalidCorrection => "invalid_correction",
            Self::NoFeasibleDirection => "no_feasible_direction",
            Self::NoFeasibleCandidate => "no_feasible_candidate",
            Self::NoResidualProgress => "no_residual_progress",
            Self::OriginalGateRejected => "original_gate_rejected",
            Self::RequestedLimitsRejected => "requested_limits_rejected",
            Self::ExecutionFailure => "execution_failure",
            Self::Cancelled => "cancelled",
            Self::UnclassifiedGateFailure => "unclassified_gate_failure",
        }
    }

    pub(crate) fn cacheable(self) -> bool {
        !matches!(
            self,
            Self::UnsupportedNetwork
                | Self::ExecutionFailure
                | Self::Cancelled
                | Self::UnclassifiedGateFailure
        )
    }
}

/// Only known deterministic preservation failures may enter a negative cache.
/// Unknown future kinds and cancellation retain their original retry semantics.
pub(crate) fn cacheable_violation(kind: &str) -> bool {
    matches!(
        kind,
        "invalid_baseline"
            | "relation"
            | "candidate_topology"
            | "stale_candidate"
            | "detached_junction"
            | "lost_facet"
            | "collapsed_edge"
            | "shape_limit"
            | "girdle_gauge"
            | "unrepresentable"
            | "projection_failed"
            | "at_surfaces"
    )
}

/// CPU fallback work grouped by the accelerated projection's reason for stopping.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProjectionFallbackDiagnostics {
    /// Actual calls to the original CPU projection, including currently active calls.
    pub calls: u64,
    /// Completed original CPU projections that returned a candidate.
    pub succeeded: u64,
    /// Completed original CPU projections that returned a preservation violation.
    pub refused: u64,
    /// Completed fallback service time; different workers' intervals can overlap.
    pub service_ms: f64,
    /// Portion of service time in completed calls that returned a candidate.
    pub succeeded_service_ms: f64,
    /// Portion of service time in completed calls that returned a refusal.
    pub refused_service_ms: f64,
    /// Exact request cache hits returning a previously certified CPU result.
    pub cached_successes: u64,
    /// Exact request cache hits returning a deterministic previous refusal.
    pub cached_refusals: u64,
}

#[derive(Clone, Default)]
pub(super) struct ClassifiedCounts {
    pub misses: BTreeMap<String, u64>,
    pub fallbacks: BTreeMap<String, ProjectionFallbackDiagnostics>,
    pub original_gate_rejection_kinds: BTreeMap<String, u64>,
    pub original_gate_rejection_examples: BTreeMap<String, String>,
    pub fallback_rejection_kinds: BTreeMap<String, u64>,
    pub fallback_rejection_examples: BTreeMap<String, String>,
    pub fallback_failure_stages: BTreeMap<String, u64>,
    pub gate_phases: BTreeMap<String, ProjectionGateDiagnostics>,
}

impl ClassifiedCounts {
    pub fn miss(&mut self, reason: ProjectionMissReason) {
        *self.misses.entry(reason.label().into()).or_default() += 1;
    }

    pub fn fallback(&mut self, reason: ProjectionMissReason) -> &mut ProjectionFallbackDiagnostics {
        self.fallbacks.entry(reason.label().into()).or_default()
    }

    pub fn gate(&mut self, phase: GatePhase, ms: f64, error: Option<&PreservationViolation>) {
        let work = self.gate_phases.entry(phase.label().into()).or_default();
        work.cpu_calls += 1;
        work.service_ms += ms;
        if let Some(error) = error {
            work.refused += 1;
            refusal(
                &mut self.original_gate_rejection_kinds,
                &mut self.original_gate_rejection_examples,
                error,
            );
        }
    }

    pub fn requested_limit(&mut self, phase: GatePhase) {
        self.gate_phases
            .entry(phase.label().into())
            .or_default()
            .requested_limit_refusals += 1;
    }

    pub fn fallback_refusal(&mut self, error: &PreservationViolation) {
        refusal(
            &mut self.fallback_rejection_kinds,
            &mut self.fallback_rejection_examples,
            error,
        );
        if let PreservationViolation::ProjectionFailed { stage, .. } = error {
            *self
                .fallback_failure_stages
                .entry(stage.chars().take(128).collect())
                .or_default() += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClassifiedCounts, ProjectionMissReason};

    #[test]
    fn fallback_outcomes_and_cache_hits_keep_the_original_miss_reason() {
        let mut counts = ClassifiedCounts::default();
        let reason = ProjectionMissReason::NoFeasibleDirection;
        counts.miss(reason);
        counts.fallback(reason).calls += 1;
        counts.fallback(reason).refused += 1;
        counts.fallback(reason).service_ms += 12.5;
        counts.miss(reason);
        counts.fallback(reason).cached_refusals += 1;
        assert_eq!(counts.misses["no_feasible_direction"], 2);
        let work = &counts.fallbacks["no_feasible_direction"];
        assert_eq!((work.calls, work.refused, work.cached_refusals), (1, 1, 1));
        assert_eq!(work.service_ms, 12.5);
        assert!(!ProjectionMissReason::ExecutionFailure.cacheable());
        assert!(!super::cacheable_violation("cancelled"));
        assert!(!super::cacheable_violation("future_backend_failure"));
        assert!(super::cacheable_violation("shape_limit"));
    }

    #[test]
    fn gate_kinds_phases_and_fallback_failures_are_separate_and_examples_are_bounded() {
        use pleochroma_cut_core::meet_preservation::PreservationViolation;
        let mut counts = ClassifiedCounts::default();
        let annotated = PreservationViolation::AtSurfaces {
            detail: "Original junction #3 would separate.".into(),
            surfaces: vec!["authored name deliberately omitted from sample".into()],
        };
        counts.gate(super::GatePhase::LineTrial, 2.5, Some(&annotated));
        counts.gate(super::GatePhase::Seed, 1.0, None);
        counts.requested_limit(super::GatePhase::Seed);
        counts.fallback_refusal(&PreservationViolation::ProjectionFailed {
            stage: "line search".into(),
            reason: "x".repeat(1000),
        });
        assert_eq!(counts.original_gate_rejection_kinds["detached_junction"], 1);
        assert_eq!(counts.gate_phases["line_trial"].cpu_calls, 1);
        assert_eq!(counts.gate_phases["line_trial"].service_ms, 2.5);
        assert_eq!(counts.gate_phases["seed"].requested_limit_refusals, 1);
        assert_eq!(counts.fallback_rejection_kinds["projection_failed"], 1);
        assert_eq!(counts.fallback_failure_stages["line search"], 1);
        assert!(
            !counts.original_gate_rejection_examples["detached_junction"].contains("authored name")
        );
        assert_eq!(
            counts.fallback_rejection_examples["projection_failed"]
                .chars()
                .count(),
            384
        );
        for error in [
            PreservationViolation::ShapeLimit,
            PreservationViolation::Unrepresentable,
            PreservationViolation::StaleCandidate,
            PreservationViolation::Cancelled,
        ] {
            counts.gate(super::GatePhase::FinalIterate, 0.0, Some(&error));
        }
        assert_eq!(counts.original_gate_rejection_examples.len(), 4);
    }
}
