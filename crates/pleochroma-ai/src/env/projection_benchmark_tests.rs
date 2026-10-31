//! CPU-only contract checks for the paired diagnostic harness; no CUDA device required.
use super::*;

fn fixture() -> Design {
    Design::concave_fixture()
}

#[test]
fn generated_requests_are_bounded_deterministic_and_respect_the_movable_scope() {
    let design = fixture();
    let config = ProjectionProbeConfig::default();
    let actions = projection_probe_actions(&design, &config).expect("movable fixture");
    let space = config.measure(&design).unwrap();
    assert_eq!(actions, projection_probe_actions(&design, &config).unwrap());
    assert_eq!(actions.len(), 10);
    for action in &actions {
        assert_eq!(action.deltas_deg.len(), design.tiers.len());
        for (tier, delta) in action.deltas_deg.iter().enumerate() {
            assert!(delta.abs() <= config.max_step_deg);
            assert!(space.is_free(tier) || *delta == 0.0);
        }
    }
    let scoped = ProjectionProbeConfig {
        only_tiers: Some(BTreeSet::from([2])),
        ..config
    };
    let actions = projection_probe_actions(&design, &scoped).unwrap();
    assert_eq!(actions.len(), 4);
    assert!(actions.iter().all(|action| {
        action
            .deltas_deg
            .iter()
            .enumerate()
            .all(|(tier, delta)| tier == 2 || *delta == 0.0)
    }));
}

#[test]
fn zero_action_reports_noop_validation_and_phase_boundaries() {
    let design = fixture();
    let action = ContinuousTierAction::new(vec![0.0; design.tiers.len()]);
    let mut events = Vec::new();
    let report = run_projection_probe_with_events(
        &design,
        &[action.clone()],
        &ProjectionProbeConfig::default(),
        None,
        |event| events.push(event.clone()),
    )
    .unwrap();
    assert_eq!(report.candidate_route, "cpu_reference");
    assert!(report.geometry_before.is_none() && report.geometry_after.is_none());
    let case = &report.actions[0];
    assert_eq!(case.action, action);
    assert_eq!(case.repeat_matches_first, vec![true, true]);
    let first = case.candidate_first.as_ref().unwrap();
    assert!(first.accepted && first.returned_noop);
    assert_eq!(first.original_gate_accepts, Some(true));
    assert_eq!(first.validation_passed, Some(true));
    assert_eq!(
        case.comparison.as_ref().unwrap().exact_output_equal,
        Some(true)
    );
    let phases: Vec<_> = events
        .iter()
        .map(|event| (event.phase, event.state, event.repeat_index))
        .collect();
    assert_eq!(
        phases,
        vec![
            ("proposal", "started", None),
            ("proposal", "completed", None),
            ("cpu_reference", "started", None),
            ("cpu_reference", "completed", None),
            ("candidate_repeat", "started", Some(0)),
            ("candidate_repeat", "completed", Some(0)),
            ("candidate_repeat", "started", Some(1)),
            ("candidate_repeat", "completed", Some(1)),
        ]
    );
    assert!(
        events
            .iter()
            .filter(|event| event.state == "started")
            .all(|event| event.attempt.is_none())
    );
}

#[test]
fn proposal_failure_emits_a_completed_event_without_running_either_route() {
    let design = fixture();
    let mut events = Vec::new();
    let report = run_projection_probe_with_events(
        &design,
        &[ContinuousTierAction::new(Vec::new())],
        &ProjectionProbeConfig::default(),
        None,
        |event| events.push(event.clone()),
    )
    .unwrap();
    let case = &report.actions[0];
    assert!(
        case.proposal_error
            .as_ref()
            .unwrap()
            .contains("WrongActionLength")
    );
    assert!(case.cpu_reference.is_none() && case.candidate_first.is_none());
    assert!(case.candidate_repeats.is_empty());
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].state, "completed");
    assert_eq!(events[1].proposal_error, case.proposal_error);
}

#[test]
fn paired_cpu_attempt_uses_the_same_proposal_and_production_projection() {
    let design = fixture();
    let config = ProjectionProbeConfig {
        max_step_deg: 0.1,
        repeats: 0,
        ..Default::default()
    };
    let space = config.measure(&design).unwrap();
    let mut deltas = vec![0.0; design.tiers.len()];
    deltas[2] = 0.05;
    let action = ContinuousTierAction::new(deltas);
    let expected = moves::apply_action(&space, &design, &action, config.max_step_deg)
        .expect("existing feasible fixture request");
    let proposal = moves::propose(&space, &design, &action, config.max_step_deg).unwrap();
    let (report, candidate) = attempt(&space, &design, &proposal, &config, None).unwrap();
    assert!(same_design_state(candidate.as_ref().unwrap(), &expected));
    assert!(report.accepted && !report.returned_noop);
    assert_eq!(report.validation_passed, Some(true));
}

#[test]
fn zero_action_outside_requested_bounds_is_reported_without_changing_acceptance() {
    let design = fixture();
    let angle = design.tiers[2].angle_deg;
    let config = ProjectionProbeConfig {
        angle_bounds: BTreeMap::from([(2, (angle + 0.1, angle + 0.2))]),
        repeats: 1,
        ..Default::default()
    };
    let report = run_projection_probe(
        &design,
        &[ContinuousTierAction::new(vec![0.0; design.tiers.len()])],
        &config,
        None,
    )
    .unwrap();
    let case = &report.actions[0];
    let first = case.candidate_first.as_ref().unwrap();
    assert!(
        first.accepted && first.returned_noop,
        "keep the actual production outcome"
    );
    assert_eq!(first.original_gate_accepts, Some(true));
    assert_eq!(first.requested_bounds_accepts, Some(false));
    assert_eq!(first.validation_passed, Some(false));
    assert_eq!(case.repeat_matches_first, vec![false]);
}

#[test]
fn nonfinite_comparison_cannot_look_like_a_zero_difference() {
    let reference = fixture();
    let mut candidate = reference.clone();
    candidate.tiers[2].angle_deg = f64::NAN;
    let report = comparison(Some(&reference), Some(&candidate));
    assert_eq!(report.finite_differences, Some(false));
    assert_eq!(report.exact_output_equal, Some(false));
    assert!(report.max_angle_difference_deg.is_none());
    assert!(report.max_pinned_mast_difference.is_none());
}

#[test]
fn cached_gate_refusal_retains_its_kind_and_bounded_reason() {
    let error = pleochroma_cut_core::meet_preservation::PreservationViolation::AtSurfaces {
        detail: "Original junction #3 would separate.".into(),
        surfaces: vec!["private authored surface name".into()],
    };
    let fresh = ProjectionRefusal::Gate(error);
    let cached = ProjectionRefusal::CachedGate(fresh.to_string());
    assert_eq!(refusal(&fresh), refusal(&cached));
    assert_eq!(refusal(&cached).0, "detached_junction");
    assert!(!refusal(&cached).1.contains("private authored"));
}

#[test]
fn invalid_probe_settings_and_no_movable_tiers_are_setup_errors() {
    let design = fixture();
    for config in [
        ProjectionProbeConfig {
            max_step_deg: f64::NAN,
            ..Default::default()
        },
        ProjectionProbeConfig {
            repeats: 17,
            ..Default::default()
        },
        ProjectionProbeConfig {
            vary_anchored: false,
            ..Default::default()
        },
        ProjectionProbeConfig {
            angle_bounds: BTreeMap::from([(design.tiers.len(), (0.0, 1.0))]),
            ..Default::default()
        },
    ] {
        assert!(run_projection_probe(&design, &[], &config, None).is_err());
    }
}
