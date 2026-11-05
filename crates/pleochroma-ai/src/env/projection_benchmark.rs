//! Paired projection diagnostics on caller-supplied real designs and fixed actions.
//! No search rules are changed; initialization, attempts and validation are timed separately.
use super::{
    ContinuousTierAction,
    moves::{self, MoveSpace, ProjectionRefusal},
};
use crate::gpu_geometry::{GeometryDiagnostics, GpuGeometry, diagnostic_reason, same_design_state};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{Design, meet_preservation::PreservationPolicy};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

/// Settings passed unchanged into the ordinary environment move/projection path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionProbeConfig {
    /// Maximum requested per-tier angle step in degrees.
    pub max_step_deg: f64,
    /// Whether pinned tiers can turn about their measured hinges.
    pub vary_anchored: bool,
    /// Optional movable-tier restriction.
    pub only_tiers: Option<BTreeSet<usize>>,
    /// Original authoritative preservation policy.
    pub preservation_policy: PreservationPolicy,
    /// Requested per-tier angle intervals.
    pub angle_bounds: BTreeMap<usize, (f64, f64)>,
    /// Exact repeats following the first selected-route attempt, at most sixteen.
    pub repeats: usize,
}

impl Default for ProjectionProbeConfig {
    fn default() -> Self {
        Self {
            max_step_deg: 0.5,
            vary_anchored: true,
            only_tiers: None,
            preservation_policy: PreservationPolicy::OPTIMIZE,
            angle_bounds: BTreeMap::new(),
            repeats: 2,
        }
    }
}

impl ProjectionProbeConfig {
    fn measure(&self, design: &Design) -> Result<MoveSpace, String> {
        if !self.max_step_deg.is_finite() || self.max_step_deg <= 0.0 || self.repeats > 16 {
            return Err(
                "projection probe requires a positive finite step and at most sixteen repeats"
                    .into(),
            );
        }
        if self.angle_bounds.iter().any(|(&tier, &(low, high))| {
            tier >= design.tiers.len() || !low.is_finite() || !high.is_finite() || low > high
        }) {
            return Err("projection probe contains an invalid requested angle interval".into());
        }
        MoveSpace::measure_with(
            design,
            self.vary_anchored,
            self.only_tiers.as_ref(),
            self.preservation_policy,
            &self.angle_bounds,
        )
        .map_err(|error| error.to_string())
    }
}

/// One measured attempt, followed by independent original-gate verification.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionAttemptReport {
    /// Host wall time of the projection call, excluding verification.
    pub wall_ms: f64,
    /// Whether the production projection returned a candidate.
    pub accepted: bool,
    /// Whether that candidate exactly equals the input's complete authored state.
    pub returned_noop: bool,
    /// Stable refusal category, including the original kind of memoized gate refusals.
    pub refusal_kind: Option<String>,
    /// Bounded refusal text, with no full design or surface-name list.
    pub refusal: Option<String>,
    /// Time spent independently validating the original gate, finiteness and bounds.
    pub verification_wall_ms: f64,
    /// The independent original gate result; absent when projection returned no candidate.
    pub original_gate_accepts: Option<bool>,
    /// Stable independent original-gate refusal kind, if that gate refused.
    pub original_gate_refusal_kind: Option<String>,
    /// Bounded independent-validation error text if that gate refused.
    pub original_gate_refusal: Option<String>,
    /// Whether all returned angles and authored pinned masts are finite.
    pub finite_authored_parameters: Option<bool>,
    /// Whether the result satisfies every requested interval with production tolerances.
    pub requested_bounds_accepts: Option<bool>,
    /// Whether every movable tier stays within the production per-step limit.
    pub step_limit_accepts: Option<bool>,
    /// Conjunction of the independent gate, finiteness, requested-bound and step checks.
    pub validation_passed: Option<bool>,
    /// Cumulative geometry counters after the attempt; verification is excluded.
    pub geometry_after: Option<GeometryDiagnostics>,
}

/// CPU/CUDA output differences are measurements, not automatic test failures.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionComparison {
    /// Whether both routes accepted or both refused.
    pub same_acceptance: bool,
    /// Complete-state exact equality, only when both routes accepted.
    pub exact_output_equal: Option<bool>,
    /// Whether corresponding angle/mast differences are finite and tier counts agree.
    pub finite_differences: Option<bool>,
    /// Largest absolute difference in returned tier angles, when both accepted.
    pub max_angle_difference_deg: Option<f64>,
    /// Largest difference between corresponding pinned masts, when both accepted.
    pub max_pinned_mast_difference: Option<f64>,
}

/// One fixed action applied independently to the same original/current input design.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionActionReport {
    /// Exact input request vector in tier order.
    pub action: ContinuousTierAction,
    /// Proposal/safety error before either projection route was called.
    pub proposal_error: Option<String>,
    /// Original CPU implementation on this action's single prepared proposal.
    pub cpu_reference: Option<ProjectionAttemptReport>,
    /// First CUDA-backed attempt, or the CPU reference when CUDA was not requested.
    pub candidate_first: Option<ProjectionAttemptReport>,
    /// Repeated identical requests on the same prepared selected-route space.
    pub candidate_repeats: Vec<ProjectionAttemptReport>,
    /// CPU reference versus first selected-route result.
    pub comparison: Option<ProjectionComparison>,
    /// Exact repeated refusal or identical accepted output passing all validation checks.
    pub repeat_matches_first: Vec<bool>,
}

/// A serial paired reproducer; all service counters come from the supplied geometry owner.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionProbeReport {
    /// Configuration used for both routes.
    pub config: ProjectionProbeConfig,
    /// Number of authored tiers in the supplied design.
    pub tier_count: usize,
    /// `cuda_geometry` when a geometry owner was supplied, otherwise `cpu_reference`.
    pub candidate_route: &'static str,
    /// Original baseline, hinges, solved tiers and move-space preparation time.
    pub move_space_setup_ms: f64,
    /// Episode CUDA-network preparation time; device initialization happens in the caller.
    pub cuda_prepare_ms: f64,
    /// Counters before this design's CUDA network was attached.
    pub geometry_before: Option<GeometryDiagnostics>,
    /// Counters after every fixed action and repeat.
    pub geometry_after: Option<GeometryDiagnostics>,
    /// Explicit action requests, outcomes, timings and comparisons.
    pub actions: Vec<ProjectionActionReport>,
}

/// A caller-visible phase boundary that can be flushed before a remote run times out.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionProbeEvent {
    /// Zero-based action index in this design's request set.
    pub action_index: usize,
    /// Exact request vector, included before any potentially expensive projection.
    pub action: ContinuousTierAction,
    /// `proposal`, `cpu_reference`, `candidate_first` or `candidate_repeat`.
    pub phase: &'static str,
    /// `started` before the call or `completed` after its result/verification.
    pub state: &'static str,
    /// Zero-based repeat index for a candidate-repeat event.
    pub repeat_index: Option<usize>,
    /// Completed projection result; absent at phase entry or proposal failure.
    pub attempt: Option<ProjectionAttemptReport>,
    /// Proposal error when either route could not be called.
    pub proposal_error: Option<String>,
}

impl ProjectionProbeEvent {
    fn started(
        index: usize,
        action: &ContinuousTierAction,
        phase: &'static str,
        repeat_index: Option<usize>,
    ) -> Self {
        Self {
            action_index: index,
            action: action.clone(),
            phase,
            state: "started",
            repeat_index,
            attempt: None,
            proposal_error: None,
        }
    }
}

fn actions_for(space: &MoveSpace, tiers: usize, max_step: f64) -> Vec<ContinuousTierAction> {
    let mut actions = Vec::new();
    for &tier in space.free().iter().take(4) {
        for sign in [1.0, -1.0] {
            let mut deltas = vec![0.0; tiers];
            deltas[tier] = sign * max_step;
            actions.push(ContinuousTierAction::new(deltas));
        }
    }
    for magnitude in [max_step.min(0.1), max_step] {
        let mut deltas = vec![0.0; tiers];
        for (position, &tier) in space.free().iter().enumerate() {
            deltas[tier] = if position % 2 == 0 {
                magnitude
            } else {
                -magnitude
            };
        }
        actions.push(ContinuousTierAction::new(deltas));
    }
    actions
}

/// Generate at most ten deterministic requests using the actual movable-tier set.
///
/// # Errors
/// Returns invalid settings, invalid baseline, or no-movable-tier errors.
pub fn projection_probe_actions(
    design: &Design,
    config: &ProjectionProbeConfig,
) -> Result<Vec<ContinuousTierAction>, String> {
    let space = config.measure(design)?;
    Ok(actions_for(&space, design.tiers.len(), config.max_step_deg))
}

fn refusal(error: &ProjectionRefusal) -> (String, String) {
    let (kind, detail) = match error {
        ProjectionRefusal::Gate(error) => (error.kind(), diagnostic_reason(error)),
        ProjectionRefusal::GpuFailure(error) => ("gpu_failure", error.clone()),
        ProjectionRefusal::CachedGate(error) => error
            .strip_prefix("meet-preserving gate (")
            .and_then(|text| text.split_once("): "))
            .map_or(("cached_gate", error.clone()), |(kind, detail)| {
                (kind, detail.to_owned())
            }),
        ProjectionRefusal::AcceleratedMiss(reason) => (reason.label(), error.to_string()),
        ProjectionRefusal::OutsideRequestedBounds => {
            ("outside_requested_bounds", error.to_string())
        }
        ProjectionRefusal::ExceedsStep => ("exceeds_step", error.to_string()),
    };
    let detail = detail
        .split_once(" Affected surfaces:")
        .map_or(detail.as_str(), |(text, _)| text);
    (kind.into(), detail.chars().take(384).collect())
}

fn verify_candidate(
    report: &mut ProjectionAttemptReport,
    candidate: &Design,
    space: &MoveSpace,
    design: &Design,
    config: &ProjectionProbeConfig,
) {
    report.returned_noop = same_design_state(candidate, design);
    let started = Instant::now();
    let verification = space
        .baseline()
        .validate(candidate, config.preservation_policy);
    report.original_gate_accepts = Some(verification.is_ok());
    report.original_gate_refusal_kind =
        verification.as_ref().err().map(|error| error.kind().into());
    report.original_gate_refusal = verification.as_ref().err().map(diagnostic_reason);
    let finite = candidate.tiers.iter().all(|tier| {
        tier.angle_deg.is_finite()
            && match &tier.constraint {
                MeetConstraint::ScaleReference(mast) => mast.is_finite(),
                _ => true,
            }
    });
    let bounds_ok = config.angle_bounds.iter().all(|(&tier, &(low, high))| {
        candidate.tiers.get(tier).is_some_and(|tier| {
            tier.angle_deg.is_finite()
                && tier.angle_deg >= low - 1e-9
                && tier.angle_deg <= high + 1e-9
        })
    });
    let step_ok = space.free().iter().all(|&tier| {
        candidate.tiers.get(tier).is_some_and(|returned| {
            let delta = returned.angle_deg - design.tiers[tier].angle_deg;
            delta.is_finite() && delta.abs() <= config.max_step_deg + 1e-9
        })
    });
    report.finite_authored_parameters = Some(finite);
    report.requested_bounds_accepts = Some(bounds_ok);
    report.step_limit_accepts = Some(step_ok);
    report.validation_passed = Some(verification.is_ok() && finite && bounds_ok && step_ok);
    report.verification_wall_ms = started.elapsed().as_secs_f64() * 1e3;
}

fn attempt(
    space: &MoveSpace,
    design: &Design,
    proposed: &Design,
    config: &ProjectionProbeConfig,
    geometry: Option<&Arc<GpuGeometry>>,
) -> Result<(ProjectionAttemptReport, Option<Design>), String> {
    if let Some(geometry) = geometry {
        geometry.check()?;
    }
    let started = Instant::now();
    let result = if proposed == design {
        Ok(design.clone())
    } else {
        moves::project(space, design, proposed, config.max_step_deg)
    };
    let wall_ms = started.elapsed().as_secs_f64() * 1e3;
    if let Some(geometry) = geometry {
        geometry.check()?;
    }
    let mut report = ProjectionAttemptReport {
        wall_ms,
        accepted: result.is_ok(),
        returned_noop: false,
        refusal_kind: None,
        refusal: None,
        verification_wall_ms: 0.0,
        original_gate_accepts: None,
        original_gate_refusal_kind: None,
        original_gate_refusal: None,
        finite_authored_parameters: None,
        requested_bounds_accepts: None,
        step_limit_accepts: None,
        validation_passed: None,
        geometry_after: geometry.map(|owner| owner.diagnostics()),
    };
    match result {
        Ok(candidate) => {
            verify_candidate(&mut report, &candidate, space, design, config);
            Ok((report, Some(candidate)))
        }
        Err(error) => {
            let (kind, reason) = refusal(&error);
            report.refusal_kind = Some(kind);
            report.refusal = Some(reason);
            Ok((report, None))
        }
    }
}

fn comparison(reference: Option<&Design>, candidate: Option<&Design>) -> ProjectionComparison {
    let both = reference.zip(candidate);
    let finite = both.map(|(a, b)| {
        a.tiers.len() == b.tiers.len()
            && a.tiers.iter().zip(&b.tiers).all(|(a, b)| {
                (a.angle_deg - b.angle_deg).is_finite()
                    && match (&a.constraint, &b.constraint) {
                        (MeetConstraint::ScaleReference(a), MeetConstraint::ScaleReference(b)) => {
                            (a - b).is_finite()
                        }
                        (MeetConstraint::ScaleReference(_), _)
                        | (_, MeetConstraint::ScaleReference(_)) => false,
                        _ => true,
                    }
            })
    });
    let finite_both = both.filter(|_| finite == Some(true));
    ProjectionComparison {
        same_acceptance: reference.is_some() == candidate.is_some(),
        exact_output_equal: both.map(|(a, b)| same_design_state(a, b)),
        finite_differences: finite,
        max_angle_difference_deg: finite_both.map(|(a, b)| {
            a.tiers
                .iter()
                .zip(&b.tiers)
                .map(|(a, b)| (a.angle_deg - b.angle_deg).abs())
                .fold(0.0, f64::max)
        }),
        max_pinned_mast_difference: finite_both.and_then(|(a, b)| {
            a.tiers
                .iter()
                .zip(&b.tiers)
                .filter_map(|(a, b)| match (&a.constraint, &b.constraint) {
                    (MeetConstraint::ScaleReference(a), MeetConstraint::ScaleReference(b)) => {
                        Some((a - b).abs())
                    }
                    _ => None,
                })
                .reduce(f64::max)
        }),
    }
}

struct ProbeContext<'a> {
    design: &'a Design,
    config: &'a ProjectionProbeConfig,
    cpu: &'a MoveSpace,
    selected: &'a MoveSpace,
    geometry: Option<&'a Arc<GpuGeometry>>,
}

impl ProbeContext<'_> {
    fn observed_attempt(
        &self,
        proposed: &Design,
        selected: bool,
        mut event: ProjectionProbeEvent,
        observer: &mut impl FnMut(&ProjectionProbeEvent),
    ) -> Result<(ProjectionAttemptReport, Option<Design>), String> {
        observer(&event);
        let result = attempt(
            if selected { self.selected } else { self.cpu },
            self.design,
            proposed,
            self.config,
            if selected { self.geometry } else { None },
        )?;
        event.state = "completed";
        event.attempt = Some(result.0.clone());
        observer(&event);
        Ok(result)
    }

    fn action(
        &self,
        index: usize,
        action: ContinuousTierAction,
        observer: &mut impl FnMut(&ProjectionProbeEvent),
    ) -> Result<ProjectionActionReport, String> {
        let mut report = ProjectionActionReport {
            action,
            proposal_error: None,
            cpu_reference: None,
            candidate_first: None,
            candidate_repeats: Vec::new(),
            comparison: None,
            repeat_matches_first: Vec::new(),
        };
        let mut event = ProjectionProbeEvent::started(index, &report.action, "proposal", None);
        observer(&event);
        let proposal = moves::propose(
            self.cpu,
            self.design,
            &report.action,
            self.config.max_step_deg,
        );
        event.state = "completed";
        event.proposal_error = proposal.as_ref().err().map(|error| format!("{error:?}"));
        observer(&event);
        let proposed = match proposal {
            Ok(proposed) => proposed,
            Err(_) => {
                report.proposal_error = event.proposal_error;
                return Ok(report);
            }
        };
        let event = ProjectionProbeEvent::started(index, &report.action, "cpu_reference", None);
        let (reference_report, reference) =
            self.observed_attempt(&proposed, false, event, observer)?;
        let (first_report, first) = if self.geometry.is_some() {
            let event =
                ProjectionProbeEvent::started(index, &report.action, "candidate_first", None);
            self.observed_attempt(&proposed, true, event, observer)?
        } else {
            (reference_report.clone(), reference.clone())
        };
        report.comparison = Some(comparison(reference.as_ref(), first.as_ref()));
        for repeat in 0..self.config.repeats {
            let event = ProjectionProbeEvent::started(
                index,
                &report.action,
                "candidate_repeat",
                Some(repeat),
            );
            let (repeated_report, repeated) =
                self.observed_attempt(&proposed, true, event, observer)?;
            report
                .repeat_matches_first
                .push(match (first.as_ref(), repeated.as_ref()) {
                    (Some(a), Some(b)) => {
                        first_report.validation_passed == Some(true)
                            && repeated_report.validation_passed == Some(true)
                            && same_design_state(a, b)
                    }
                    (None, None) => {
                        first_report.refusal_kind == repeated_report.refusal_kind
                            && first_report.refusal == repeated_report.refusal
                    }
                    _ => false,
                });
            report.candidate_repeats.push(repeated_report);
        }
        report.cpu_reference = Some(reference_report);
        report.candidate_first = Some(first_report);
        Ok(report)
    }
}

/// Compare CPU and optional CUDA projection on identical prepared proposals.
///
/// Empty `actions` generates the bounded default set after one move-space preparation.
/// Each action starts from `design`; neither route's result becomes the next action's input.
/// The first selected attempt precedes its explicit repeats. Equivalent earlier requests
/// (including requests clamped to the same proposal) can already have warmed its caches.
/// Call with a dedicated geometry owner to keep cumulative diagnostic snapshots isolated.
///
/// # Errors
/// Returns setup/configuration errors (including designs with no movable tiers) or fatal
/// CUDA errors, never disguising them as refusals. This also applies to explicit zero actions.
pub fn run_projection_probe(
    design: &Design,
    actions: &[ContinuousTierAction],
    config: &ProjectionProbeConfig,
    geometry: Option<Arc<GpuGeometry>>,
) -> Result<ProjectionProbeReport, String> {
    run_projection_probe_with_events(design, actions, config, geometry, |_| {})
}

/// Paired projection diagnostics with callbacks before and after each action phase.
///
/// # Errors
/// Returns the same setup, configuration and execution errors as [`run_projection_probe`].
pub fn run_projection_probe_with_events(
    design: &Design,
    actions: &[ContinuousTierAction],
    config: &ProjectionProbeConfig,
    geometry: Option<Arc<GpuGeometry>>,
    mut on_event: impl FnMut(&ProjectionProbeEvent),
) -> Result<ProjectionProbeReport, String> {
    let started = Instant::now();
    let cpu = config.measure(design)?;
    let move_space_setup_ms = started.elapsed().as_secs_f64() * 1e3;
    let geometry_before = geometry.as_ref().map(|owner| owner.diagnostics());
    let started = Instant::now();
    let mut selected = cpu.clone();
    if let Some(owner) = &geometry {
        owner.check()?;
        selected.set_gpu_geometry(Arc::clone(owner));
        owner.check()?;
    }
    let cuda_prepare_ms = if geometry.is_some() {
        started.elapsed().as_secs_f64() * 1e3
    } else {
        0.0
    };
    let requests = if actions.is_empty() {
        actions_for(&cpu, design.tiers.len(), config.max_step_deg)
    } else {
        actions.to_vec()
    };
    let context = ProbeContext {
        design,
        config,
        cpu: &cpu,
        selected: &selected,
        geometry: geometry.as_ref(),
    };
    let reports = requests
        .into_iter()
        .enumerate()
        .map(|(index, action)| context.action(index, action, &mut on_event))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProjectionProbeReport {
        config: config.clone(),
        tier_count: design.tiers.len(),
        candidate_route: if geometry.is_some() {
            "cuda_geometry"
        } else {
            "cpu_reference"
        },
        move_space_setup_ms,
        cuda_prepare_ms,
        geometry_before,
        geometry_after: geometry.as_ref().map(|owner| owner.diagnostics()),
        actions: reports,
    })
}

#[cfg(test)]
#[path = "projection_benchmark_tests.rs"]
mod tests;
