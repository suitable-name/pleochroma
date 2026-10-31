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
        &[2],
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
