//! CPU-reference regressions for the accelerated path's host policy/caching.
//! These tests need no CUDA context; hardware parity coverage remains separate.
use super::{Design, GpuGeometry, Projection, ProjectionMissReason, TrustRegion};
use pleochroma_cut_core::meet_preservation::{MeetPreservationBaseline, PreservationPolicy};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

fn prepared() -> (Design, Arc<Projection>, Arc<GpuGeometry>) {
    let mut design = Design::concave_fixture();
    design.concave_tiers.clear();
    design.concave_tier_ids.clear();
    prepared_design(design, 2)
}

fn prepared_design(design: Design, free: usize) -> (Design, Arc<Projection>, Arc<GpuGeometry>) {
    let baseline = Arc::new(MeetPreservationBaseline::capture(&design).expect("valid fixture"));
    let backend = Arc::new(GpuGeometry {
        counters: Default::default(),
        failure: Mutex::new(None),
        unsupported: Mutex::new(BTreeMap::new()),
        solid: None,
    });
    let solved = design.solve().expect("authored fixture");
    let projection = Projection::prepare(
        &design,
        &[free],
        &BTreeSet::new(),
        false,
        None,
        baseline,
        PreservationPolicy::RETARGET,
        backend.clone(),
        Some(&solved),
    )
    .expect("supported fixture");
    (design, projection, backend)
}

#[test]
fn exact_failed_request_is_counted_but_not_recomputed_and_health_precedes_cache() {
    let (design, projection, backend) = prepared();
    let angle = design.tiers[2].angle_deg;
    let locked = BTreeMap::from([(2, (angle, angle))]);
    let mut outside = design.clone();
    outside.tiers[2].angle_deg += 0.1;
    for _ in 0..2 {
        assert_eq!(
            projection.project_classified(&outside, &locked),
            Err(ProjectionMissReason::InvalidSeed)
        );
    }
    let counts = backend.diagnostics();
    assert_eq!(counts.projection_requests, 2);
    assert_eq!(counts.projection_miss_reasons["invalid_seed"], 2);
    assert_eq!(counts.projection_miss_cache_hits, 1);
    assert_eq!(counts.cpu_gate_calls, 0);
    backend.fail("injected infrastructure failure".into());
    assert_eq!(
        projection.project_classified(&outside, &locked),
        Err(ProjectionMissReason::ExecutionFailure)
    );
    let after = backend.diagnostics();
    assert_eq!(after.projection_miss_cache_hits, 1);
    assert_eq!(after.projection_miss_reasons["execution_failure"], 1);
}

#[test]
fn requested_limit_refusal_does_not_become_a_negative_original_gate_certificate() {
    let (design, projection, backend) = prepared();
    let (mut facts, solved) = projection
        .baseline
        .validate_and_solve_with(&design, projection.policy, None)
        .expect("original certificate");
    // A narrow requested allowance can reject an otherwise original-policy
    // certified fact. Inject only that measured distance to isolate cache scope.
    let mut candidate = design.clone();
    candidate.tiers[2].angle_deg += 0.1;
    let travel = projection.network.turn_travel(&design, &candidate);
    let tight = projection.effective_policy(&candidate, 0.0);
    let wide = projection.effective_policy(&candidate, travel);
    assert!(wide.max_junction_displacement > tight.max_junction_displacement);
    facts.shape_distance = 0.5 * (tight.max_junction_displacement + wide.max_junction_displacement);
    projection.certificates.insert(&candidate, facts, solved);
    assert_eq!(
        projection.valid(&candidate, 0.0),
        Err(ProjectionMissReason::RequestedLimitsRejected)
    );
    assert!(projection.certificates.rejection(&candidate).is_none());
    assert_eq!(projection.valid(&candidate, travel), Ok(()));
    let counts = backend.diagnostics();
    assert_eq!(counts.original_gate_refusals, 0);
    assert_eq!(counts.requested_limit_refusals, 1);
    assert_eq!(counts.cpu_gate_calls, 0);
}

#[test]
fn deterministic_original_gate_refusal_is_cached_without_counting_a_second_gate() {
    let (mut design, projection, backend) = prepared();
    design.tiers.pop();
    assert_eq!(
        projection.original_facts(&design).unwrap_err().0,
        ProjectionMissReason::OriginalGateRejected
    );
    assert_eq!(
        projection.original_facts(&design).unwrap_err().0,
        ProjectionMissReason::OriginalGateRejected
    );
    let counts = backend.diagnostics();
    assert_eq!(counts.cpu_gate_calls, 1);
    assert_eq!(counts.original_gate_refusals, 1);
    assert_eq!(counts.negative_gate_cache_hits, 1);
    assert_eq!(counts.requested_limit_refusals, 0);
}

#[test]
fn narrower_than_nominal_angle_room_keeps_a_useful_finite_difference() {
    let (design, projection, _) = prepared();
    let angle = design.tiers[2].angle_deg;
    let x = [angle];
    let bounds = BTreeMap::from([(2, (angle, angle + 0.0001))]);
    let legal = projection.parameter_bounds(&x, &[5.0], &bounds);
    let residual = projection
        .batch(std::slice::from_ref(&design), None)
        .unwrap()
        .pop()
        .unwrap()
        .rows
        .unwrap();
    let columns = projection
        .jacobian_columns(&design, &x, &residual, &bounds, &legal)
        .unwrap();
    assert!(
        columns[0].iter().any(|value| *value != 0.0),
        "legal production-plane change must not become a locked column"
    );
    assert!(columns[0].iter().all(|value| value.is_finite()));
}

#[test]
fn line_search_can_materialize_bounded_steps_below_one_eighth() {
    let (design, projection, _) = prepared();
    let angle = design.tiers[2].angle_deg;
    let x = [angle];
    let limits = [5.0];
    let legal = [(angle, angle + 0.1)];
    let bounds = BTreeMap::from([(2, legal[0])]);
    let trust = || TrustRegion {
        origin: &x,
        limits: &limits,
        bounds: &bounds,
        legal: &legal,
    };
    assert!(
        projection
            .line_candidates(&design, &x, &[1.0], trust(), 0)
            .0
            .is_empty()
    );
    let (probes, values) = projection.line_candidates(&design, &x, &[1.0], trust(), 4);
    assert!(!probes.is_empty());
    assert_eq!(values[0][0], angle + 0.0625);
    assert!(
        probes
            .iter()
            .all(|candidate| (angle..=angle + 0.1).contains(&candidate.tiers[2].angle_deg))
    );
}

#[test]
fn terminal_reuses_current_rejection_even_after_negative_cache_eviction() {
    let (design, projection, backend) = prepared();
    let probe = projection
        .batch(std::slice::from_ref(&design), Some(0.0))
        .unwrap()
        .pop()
        .unwrap();
    let mut rejected = design.clone();
    rejected.tiers.pop();
    let reason = projection.valid(&rejected, 0.0).unwrap_err();
    assert_eq!(reason, ProjectionMissReason::OriginalGateRejected);
    for i in 0..16 {
        let mut other = rejected.clone();
        other.tiers[0].angle_deg += (i + 1) as f64 * 0.01;
        projection
            .certificates
            .insert_rejection(&other, "eviction".into());
    }
    assert!(projection.certificates.rejection(&rejected).is_none());
    let before = backend.diagnostics().cpu_gate_calls;
    assert_eq!(
        projection.certify_terminal(
            &rejected,
            &probe,
            Some(reason),
            0.0,
            ProjectionMissReason::NoFeasibleDirection
        ),
        Err(ProjectionMissReason::NoFeasibleDirection)
    );
    assert_eq!(backend.diagnostics().cpu_gate_calls, before);
    backend.fail("injected infrastructure failure".into());
    assert_eq!(
        projection.certify_terminal(
            &rejected,
            &probe,
            Some(reason),
            0.0,
            ProjectionMissReason::NoFeasibleDirection
        ),
        Err(ProjectionMissReason::ExecutionFailure)
    );
}

#[test]
fn improving_nonzero_residual_trial_still_uses_authoritative_gate() {
    let (design, projection, backend) = prepared();
    let angle = design.tiers[2].angle_deg;
    let x = [angle];
    let limits = [5.0];
    let bounds = BTreeMap::from([(2, (angle - 0.1, angle + 0.1))]);
    let legal = [(angle - 0.1, angle + 0.1)];
    let trust = TrustRegion {
        origin: &x,
        limits: &limits,
        bounds: &bounds,
        legal: &legal,
    };
    let (candidates, _) = projection.line_candidates(&design, &x, &[0.000001], trust, 0);
    let candidate = &candidates[0];
    let travel = projection.network.turn_travel(&design, candidate);
    let rows = projection
        .batch(std::slice::from_ref(candidate), Some(travel))
        .unwrap()
        .pop()
        .unwrap();
    assert!(rows.rows.as_ref().unwrap().iter().any(|r| *r != 0.0));
    assert!(
        projection
            .baseline
            .validate_and_solve_with(candidate, projection.policy, None)
            .is_ok()
    );
    let trust = TrustRegion {
        origin: &x,
        limits: &limits,
        bounds: &bounds,
        legal: &legal,
    };
    let mut unchecked = design.clone();
    unchecked.tiers.pop();
    let mut deferred = vec![(unchecked, 0.0)];
    let result = projection
        .search_line(
            &design,
            &x,
            &[0.000001],
            f64::INFINITY,
            travel,
            trust,
            &mut deferred,
        )
        .unwrap();
    assert!(matches!(result, super::search::LineSearch::Certified(_)));
    assert_eq!(
        deferred.len(),
        1,
        "success avoids gates for earlier deferred trials"
    );
    assert_eq!(backend.diagnostics().cpu_gate_calls, 1);
    assert_eq!(backend.diagnostics().original_gate_refusals, 0);
}

#[test]
fn residual_plateau_trial_is_certified_before_reporting_no_progress() {
    let (design, projection, backend) = prepared();
    let angle = design.tiers[2].angle_deg;
    let correction = angle.next_up() - angle;
    let x = [angle];
    let limits = [5.0];
    let legal = [(angle, angle + 0.1)];
    let bounds = BTreeMap::from([(2, legal[0])]);
    let trust = TrustRegion {
        origin: &x,
        limits: &limits,
        bounds: &bounds,
        legal: &legal,
    };
    let original_rows = projection
        .batch(std::slice::from_ref(&design), None)
        .unwrap()
        .pop()
        .unwrap()
        .rows
        .unwrap();
    let (trials, _) = projection.line_candidates(&design, &x, &[correction], trust, 0);
    let first = trials.first().expect("a represented f64 angle step");
    assert_ne!(first.tiers[2].angle_deg.to_bits(), angle.to_bits());
    let trial_rows = projection
        .batch(std::slice::from_ref(first), None)
        .unwrap()
        .pop()
        .unwrap()
        .rows
        .unwrap();
    assert_eq!(original_rows, trial_rows, "the f32 planes did not change");
    let before = original_rows.iter().map(|r| r * r).sum();
    let mut deferred = Vec::new();
    let result = projection
        .search_line(
            &design,
            &x,
            &[correction],
            before,
            0.0,
            trust,
            &mut deferred,
        )
        .expect("flat residuals do not prove an invalid move");
    let super::search::LineSearch::Certified(candidate) = result else {
        panic!("a plateau trial must be checked before no-progress failure");
    };
    assert!(super::super::certificate::same_state(&candidate, first));
    assert!(deferred.is_empty());
    let counts = backend.diagnostics();
    assert!(counts.line_trial_gates_deferred > 0);
    assert_eq!(counts.cpu_gate_calls, 1);
}

#[test]
fn imported_slack_trial_does_not_need_lower_numeric_residuals_to_be_accepted() {
    let mut design = Design::concave_fixture();
    design.concave_tiers.clear();
    design.concave_tier_ids.clear();
    // Give one of the four crown-apex facets an imported-style rounded depth.
    // The resulting miss exceeds reproduction accuracy but stays inside the
    // original identification radius, so its own slack must remain acceptable.
    let mut rounded_facet = design.tiers[4].clone();
    rounded_facet.indices = vec![design.tiers[4].indices.pop().unwrap()];
    let pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference(mast) =
        &mut rounded_facet.constraint
    else {
        panic!("pinned fixture")
    };
    *mast += 0.00002;
    design.tiers.push(rounded_facet);
    let next_id = design.tier_ids.iter().map(|id| id.0).max().unwrap() + 1;
    design.tier_ids.push(pleochroma_cut_core::TierId(next_id));
    let free = 2;
    let (design, projection, backend) = prepared_design(design, free);
    assert!(
        projection
            .network
            .junctions
            .iter()
            .any(|junction| { junction.targets.iter().any(|target| *target != 0.0) }),
        "the imported fixture must retain incidence slack targets"
    );
    let angle = design.tiers[free].angle_deg;
    let x = [angle];
    let limits = [5.0];
    let legal = [(angle, angle + 0.1)];
    let bounds = BTreeMap::from([(free, legal[0])]);
    let trust = TrustRegion {
        origin: &x,
        limits: &limits,
        bounds: &bounds,
        legal: &legal,
    };
    // A zero comparison objective forces every trial down the deferred path;
    // only the original gate may decide whether imported slack is acceptable.
    let mut deferred = Vec::new();
    let result = projection
        .search_line(
            &design,
            &x,
            &[angle.next_up() - angle],
            0.0,
            0.0,
            trust,
            &mut deferred,
        )
        .expect("imported slack can remain feasible");
    let super::search::LineSearch::Certified(candidate) = result else {
        panic!("the original preservation gate must decide imported slack");
    };
    assert!(
        projection
            .baseline
            .validate(&candidate, projection.policy)
            .is_ok()
    );
    assert_eq!(backend.diagnostics().cpu_gate_calls, 1);
}

#[test]
fn terminal_numerical_failures_check_earlier_deferred_trials_but_fatal_errors_propagate() {
    let (design, projection, backend) = prepared();
    let probe = projection
        .batch(std::slice::from_ref(&design), Some(0.0))
        .unwrap()
        .pop()
        .unwrap();
    let mut rejected = design.clone();
    rejected.tiers.pop();
    let rejection = projection.valid(&rejected, 0.0).unwrap_err();
    for reason in [
        ProjectionMissReason::InvalidLinearization,
        ProjectionMissReason::InvalidCorrection,
        ProjectionMissReason::NoFeasibleDirection,
        ProjectionMissReason::NoResidualProgress,
        ProjectionMissReason::OriginalGateRejected,
    ] {
        let mut deferred = vec![(design.clone(), 0.0)];
        let recovered = projection
            .certify_terminal_or_deferred(
                &rejected,
                &probe,
                Some(rejection),
                0.0,
                reason,
                &mut deferred,
            )
            .expect("recover earlier feasible trial");
        assert!(super::super::certificate::same_state(&recovered, &design));
        assert!(deferred.is_empty());
    }
    let before = backend.diagnostics().cpu_gate_calls;
    for reason in [
        ProjectionMissReason::ExecutionFailure,
        ProjectionMissReason::Cancelled,
        ProjectionMissReason::UnclassifiedGateFailure,
    ] {
        let mut deferred = vec![(design.clone(), 0.0)];
        assert_eq!(
            projection.certify_terminal_or_deferred(
                &rejected,
                &probe,
                Some(rejection),
                0.0,
                reason,
                &mut deferred
            ),
            Err(reason)
        );
        assert_eq!(deferred.len(), 1);
    }
    assert_eq!(backend.diagnostics().cpu_gate_calls, before);
    backend.fail("injected infrastructure failure".into());
    let mut deferred = vec![(design.clone(), 0.0)];
    assert_eq!(
        projection.certify_terminal_or_deferred(
            &rejected,
            &probe,
            Some(rejection),
            0.0,
            ProjectionMissReason::NoFeasibleDirection,
            &mut deferred
        ),
        Err(ProjectionMissReason::ExecutionFailure)
    );
    assert_eq!(deferred.len(), 1);
}

#[test]
fn unknown_numeric_rows_still_receive_the_original_gate_before_failure() {
    let (design, mut projection, backend) = prepared();
    let mutable = Arc::get_mut(&mut projection).expect("one projection owner");
    let first = mutable.network.junctions[0].basis[0];
    mutable.network.junctions[0].basis = [first; 3];
    let angle = design.tiers[2].angle_deg;
    let x = [angle];
    let limits = [5.0];
    let legal = [(angle, angle + 0.1)];
    let bounds = BTreeMap::from([(2, legal[0])]);
    let trust = TrustRegion {
        origin: &x,
        limits: &limits,
        bounds: &bounds,
        legal: &legal,
    };
    let probe = projection
        .batch(std::slice::from_ref(&design), Some(0.0))
        .unwrap()
        .pop()
        .unwrap();
    assert!(probe.rows.is_none());
    assert!(
        probe.possible,
        "an unusable numeric basis is not an impossibility proof"
    );
    let result = projection
        .search_line(
            &design,
            &x,
            &[angle.next_up() - angle],
            f64::INFINITY,
            0.0,
            trust,
            &mut Vec::new(),
        )
        .expect("use the independent original gate");
    assert!(matches!(result, super::search::LineSearch::Certified(_)));
    assert_eq!(backend.diagnostics().cpu_gate_calls, 1);
}

#[test]
fn deferred_exact_duplicates_do_not_repeat_gates_after_negative_cache_eviction() {
    let (design, projection, backend) = prepared();
    let probe = projection
        .batch(std::slice::from_ref(&design), Some(0.0))
        .unwrap()
        .pop()
        .unwrap();
    let mut rejected = Vec::new();
    for index in 0..12 {
        let mut candidate = design.clone();
        candidate.tiers.pop();
        candidate.tiers[0].angle_deg += f64::from(index) * 0.01;
        rejected.push(candidate);
    }
    let mut deferred = Vec::new();
    // Revisit the whole sequence, rather than adjacent duplicates: without the
    // request-local exact set, the second pass exceeds the eight-entry negative
    // cache and repeats every original gate during terminal recovery.
    for _ in 0..2 {
        for candidate in &rejected {
            projection.defer_line_trial(&mut deferred, candidate.clone(), 1.0);
        }
    }
    let mut other_identity = rejected[0].clone();
    other_identity.tier_ids[0].0 += 10_000;
    assert_eq!(
        other_identity, rejected[0],
        "Design equality omits tier identities"
    );
    projection.defer_line_trial(&mut deferred, other_identity, 1.0);
    projection.defer_line_trial(&mut deferred, design.clone(), 0.5);
    assert_eq!(
        deferred.len(),
        14,
        "identity changes remain distinct trials"
    );
    assert_eq!(backend.diagnostics().line_trial_gates_deferred, 14);
    let result = projection
        .certify_terminal_or_deferred(
            &design,
            &probe,
            None,
            0.0,
            ProjectionMissReason::NoResidualProgress,
            &mut deferred,
        )
        .expect("final feasible trial");
    assert!(super::super::certificate::same_state(&result, &design));
    let counts = backend.diagnostics();
    assert_eq!(
        counts.cpu_gate_calls, 1,
        "the smallest residual is certified first"
    );
    assert_eq!(counts.original_gate_refusals, 0);
    assert!(deferred.is_empty());
}

#[test]
fn a_stalled_request_certifies_at_most_the_capped_number_of_deferred_trials() {
    use super::search::MAX_DEFERRED_CERTIFICATIONS;
    let (design, projection, backend) = prepared();
    let probe = projection
        .batch(std::slice::from_ref(&design), Some(0.0))
        .unwrap()
        .pop()
        .unwrap();
    let mut deferred = Vec::new();
    for index in 0..12 {
        let mut candidate = design.clone();
        candidate.tiers.pop();
        candidate.tiers[0].angle_deg += f64::from(index) * 0.01;
        // Encounter order is the reverse of residual order.
        projection.defer_line_trial(&mut deferred, candidate, f64::from(12 - index));
    }
    // The only feasible trial has the largest residual: past the cap, never certified.
    projection.defer_line_trial(&mut deferred, design.clone(), 100.0);
    assert_eq!(
        projection.certify_terminal_or_deferred(
            &design,
            &probe,
            Some(ProjectionMissReason::OriginalGateRejected),
            0.0,
            ProjectionMissReason::NoResidualProgress,
            &mut deferred
        ),
        Err(ProjectionMissReason::NoResidualProgress)
    );
    let counts = backend.diagnostics();
    assert_eq!(counts.cpu_gate_calls, MAX_DEFERRED_CERTIFICATIONS as u64);
    assert_eq!(
        counts.original_gate_refusals,
        MAX_DEFERRED_CERTIFICATIONS as u64
    );
    assert_eq!(
        counts.deferred_trials_uncertified,
        13 - MAX_DEFERRED_CERTIFICATIONS as u64
    );
    assert!(deferred.is_empty());
}

#[test]
fn the_residual_screen_keeps_accepted_rows_and_refuses_far_junctions() {
    let (design, projection, _backend) = prepared();
    let rows = projection
        .batch(std::slice::from_ref(&design), None)
        .unwrap()
        .pop()
        .unwrap()
        .rows
        .expect("the fixture's own rows");
    assert!(
        projection.may_pass_gate(&rows),
        "the accepted original passes the screen"
    );
    // Push the first junction's containment row far past any acceptance bound.
    let first = &projection.network.junctions[0];
    let mut far = rows.clone();
    far[first.targets.len()] = 1.0;
    assert!(!projection.may_pass_gate(&far));
    assert!(
        projection.may_pass_gate(&far[1..]),
        "rows of another shape are never screened"
    );
}
