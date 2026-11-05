//! Hardware regressions for the complete cached CUDA projection. The pinned
//! octahedron and deformation are adapted from cut-core meet_preservation/tests.rs.
use super::{GpuGeometry, Projection};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{
    Design, OptimizeOptions,
    design::{ConstraintTier, ScheduleMeta, tier_hinge_points},
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    preform::PreformSpec,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn octahedron(named: bool) -> Design {
    let tier = |name: String, angle_deg, indices| ConstraintTier {
        name,
        angle_deg,
        indices,
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    };
    let mut tiers: Vec<_> = [0.0, 24.0, 48.0, 72.0]
        .into_iter()
        .enumerate()
        .map(|(i, index)| tier(format!("C{i}"), 45.0, vec![index]))
        .collect();
    tiers.push(tier("P".into(), -45.0, vec![0.0, 24.0, 48.0, 72.0]));
    if named {
        tiers[0].constraint =
            MeetConstraint::MeetNamed(vec!["C1".into(), "C2".into(), "C3".into()]);
    }
    Design::new(
        PreformSpec::block(4.0, 4.0, 4.0),
        ScheduleMeta {
            symmetry_order: 4,
            ..ScheduleMeta::standard_round_brilliant()
        },
        tiers,
    )
}

fn deformed(design: &Design) -> Design {
    let mut result = design.clone();
    for tier in &mut result.tiers {
        let theta = tier.angle_deg.abs().to_radians();
        let length = theta.sin().hypot(theta.cos() / 1.04);
        tier.angle_deg = (theta.tan() * 1.04)
            .atan()
            .to_degrees()
            .copysign(tier.angle_deg);
        if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
            *mast /= length;
        }
    }
    // Break the four-plane crown junction to require an actual correction. The
    // derived C0 of the named fixture must retain its authoring instruction.
    result.tiers[1].angle_deg += 0.2;
    result
}

fn prepared(
    design: &Design,
    backend: Arc<GpuGeometry>,
) -> (
    Arc<Projection>,
    Arc<MeetPreservationBaseline>,
    OptimizeOptions,
    BTreeMap<usize, (f64, f64)>,
) {
    let baseline =
        Arc::new(MeetPreservationBaseline::capture(design).expect("valid original meets"));
    let solved = design.solve().expect("authored solve");
    let hinges = tier_hinge_points(design, &solved);
    let mut options = OptimizeOptions {
        vary_anchored: true,
        anchor_hinges: hinges,
        preservation_baseline: Some(baseline.clone()),
        ..OptimizeOptions::default()
    };
    let free = pleochroma_cut_core::free_tier_indices_with(design, &options);
    let anchors: BTreeSet<_> = free
        .iter()
        .copied()
        .filter(|i| options.anchor_hinges.contains_key(i))
        .collect();
    let bounds: BTreeMap<_, _> = free
        .iter()
        .map(|&i| {
            (
                i,
                (
                    design.tiers[i].angle_deg - 5.0,
                    design.tiers[i].angle_deg + 5.0,
                ),
            )
        })
        .collect();
    options.angle_bounds = bounds.clone();
    let projection = Projection::prepare(
        design,
        &free,
        &anchors,
        true,
        None,
        baseline.clone(),
        PreservationPolicy::OPTIMIZE,
        backend,
        Some(&solved),
    )
    .expect("cached network supports the authoring program");
    (projection, baseline, options, bounds)
}

fn exact_planes(design: &Design) -> Vec<[u64; 4]> {
    super::network::surfaces(design)
        .expect("production planes")
        .into_iter()
        .map(|(normal, distance)| {
            [
                normal.x.to_bits(),
                normal.y.to_bits(),
                normal.z.to_bits(),
                distance.to_bits(),
            ]
        })
        .collect()
}

#[test]
#[ignore = "requires CUDA and NVRTC; checks exact negative proof caches and failure isolation"]
fn arrangement_refutations_cache_authored_and_plane_equivalent_states() {
    let design = octahedron(false);
    let backend = GpuGeometry::new_cuda(0).expect("CUDA and NVRTC");
    let (projection, _, _, _) = prepared(&design, backend.clone());
    let candidate = deformed(&design);
    let before = backend.diagnostics();
    let error = projection
        .certify(&candidate)
        .expect_err("broken crown meet");
    assert!(error.contains("no possible candidate arrangement point"));
    let first = backend.diagnostics();
    assert_eq!(first.arrangement_refusals, before.arrangement_refusals + 1);
    assert_eq!(first.cpu_gate_calls, before.cpu_gate_calls);

    assert!(projection.certify(&candidate).is_err());
    let repeated = backend.diagnostics();
    assert_eq!(repeated.arrangement_probes, first.arrangement_probes);
    assert_eq!(
        repeated.negative_gate_cache_hits,
        first.negative_gate_cache_hits + 1
    );

    let mut plateau = candidate.clone();
    plateau.tiers[1].angle_deg = plateau.tiers[1].angle_deg.next_up();
    assert_eq!(exact_planes(&plateau), exact_planes(&candidate));
    assert!(!super::certificate::same_state(&plateau, &candidate));
    assert!(projection.certify(&plateau).is_err());
    let cached = backend.diagnostics();
    assert_eq!(cached.arrangement_probes, first.arrangement_probes);
    assert_eq!(
        cached.negative_gate_cache_hits,
        repeated.negative_gate_cache_hits + 1
    );

    backend.fail("injected proof-cache infrastructure failure".into());
    let error = projection
        .certify(&plateau)
        .expect_err("health precedes cached refusal");
    assert!(error.contains("injected proof-cache infrastructure failure"));
    assert_eq!(
        backend.diagnostics().negative_gate_cache_hits,
        cached.negative_gate_cache_hits
    );
}

#[test]
#[ignore = "requires CUDA and NVRTC; checks accepted over-budget trials still use the original gate"]
fn arrangement_budget_bypass_preserves_authoritative_acceptance() {
    let tiers = [45.0, -45.0]
        .into_iter()
        .enumerate()
        .map(|(index, angle_deg)| ConstraintTier {
            name: format!("side-{index}"),
            angle_deg,
            indices: (0..32).map(|index| f64::from(index) * 3.0).collect(),
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    let design = Design::new(
        PreformSpec::block(4.0, 4.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        tiers,
    );
    let network = super::network::Network::capture(&design).expect("many-plane bicone");
    assert!(network.surfaces >= 60);
    assert!(network.arrangement_triples_within_budget().is_none());
    let backend = GpuGeometry::new_cuda(0).expect("CUDA and NVRTC");
    let (projection, baseline, _, _) = prepared(&design, backend.clone());
    let expected = baseline
        .validate(&design, PreservationPolicy::OPTIMIZE)
        .expect("original gate");
    let before = backend.diagnostics();
    assert_eq!(
        projection
            .certify(&design)
            .expect("budget bypass is not rejection"),
        expected
    );
    let first = backend.diagnostics();
    assert_eq!(
        first.arrangement_capacity_bypasses,
        before.arrangement_capacity_bypasses + 1
    );
    assert_eq!(first.arrangement_tiles, before.arrangement_tiles);
    assert_eq!(first.arrangement_triples, before.arrangement_triples);
    assert_eq!(first.cpu_gate_calls, before.cpu_gate_calls + 1);

    // The same physical planes on another authored state must remain eligible:
    // a capacity bypass or successful CPU gate is never a plane refutation.
    let mut plateau = design.clone();
    plateau.tiers[0].angle_deg = plateau.tiers[0].angle_deg.next_up();
    assert_eq!(exact_planes(&plateau), exact_planes(&design));
    projection
        .certify(&plateau)
        .expect("no cached refusal from budget bypass or success");
    let after = backend.diagnostics();
    assert_eq!(
        after.arrangement_capacity_bypasses,
        first.arrangement_capacity_bypasses + 1
    );
    assert_eq!(after.cpu_gate_calls, first.cpu_gate_calls + 1);
    assert_eq!(
        after.negative_gate_cache_hits,
        first.negative_gate_cache_hits
    );
}

fn corrected_fixture(named: bool) {
    let design = octahedron(named);
    let backend = GpuGeometry::new_cuda(0).expect("CUDA and NVRTC");
    let (projection, baseline, options, bounds) = prepared(&design, backend.clone());
    let proposal = deformed(&design);
    assert!(
        baseline
            .validate(&proposal, PreservationPolicy::OPTIMIZE)
            .is_err(),
        "exercise correction, not the already-feasible shortcut"
    );
    let reference =
        pleochroma_cut_core::optimize::project_preserved_design(&design, &proposal, &options, None)
            .expect("the original projection can repair this bounded request");
    let actual = projection
        .project(&proposal, &bounds)
        .expect("CUDA correction must find a certified bounded state");
    backend.check().expect("no latched GPU failure");
    for candidate in [&reference, &actual] {
        baseline
            .validate(candidate, PreservationPolicy::OPTIMIZE)
            .expect("original authoritative gate");
        assert_ne!(*candidate, design, "retain useful requested motion");
        for (&i, &(low, high)) in &bounds {
            assert!(
                (low..=high).contains(&candidate.tiers[i].angle_deg),
                "tier {i} outside authoring bounds"
            );
        }
    }
    if named {
        assert!(matches!(
            actual.tiers[0].constraint,
            MeetConstraint::MeetNamed(_)
        ));
    }
    let certified = projection
        .certified_solve(&actual)
        .expect("the actual returned state retained its production solve");
    assert_eq!(certified.len(), actual.tiers.len());
    let repeated_before = backend.diagnostics();
    assert_eq!(
        projection.project(&actual, &bounds),
        Some(actual.clone()),
        "the identical certified state reuses its exact gate result"
    );
    let repeated_after = backend.diagnostics();
    assert_eq!(
        repeated_before.cpu_gate_calls,
        repeated_after.cpu_gate_calls
    );
    assert_eq!(repeated_before.cuda_batches, repeated_after.cuda_batches);
    assert!(repeated_after.certificate_cache_hits > repeated_before.certificate_cache_hits);
    let mut changed = actual.clone();
    changed.tiers[1].angle_deg += 1e-10;
    assert!(
        projection.certified_solve(&changed).is_none(),
        "even a sub-plane-quantization authoring edit misses the exact-state cache"
    );
    let mut changed = actual.clone();
    changed.tier_ids[0] = pleochroma_cut_core::TierId(10_000);
    assert!(
        projection.certified_solve(&changed).is_none(),
        "tier provenance identities are part of the cache key"
    );
    let locked: BTreeMap<_, _> = bounds
        .keys()
        .map(|&i| (i, (design.tiers[i].angle_deg, design.tiers[i].angle_deg)))
        .collect();
    assert!(
        projection.project(&proposal, &locked).is_none(),
        "an out-of-bounds seed cannot bypass a zero-width authoring box"
    );
    let diagnostics = backend.diagnostics();
    assert!(
        diagnostics.cuda_batches > 0,
        "complete projection actually launched CUDA"
    );
    assert!(
        diagnostics.cuda_probes > 1,
        "finite-difference probes ran on CUDA"
    );
    assert!(
        diagnostics.line_trial_gates_avoided <= diagnostics.gpu_prefilter_refusals,
        "only conservative GPU-impossible line trials may skip the production gate"
    );
    assert_eq!(diagnostics.cpu_projection_fallbacks, 0);
    if named {
        assert!(diagnostics.cpu_authoring_probes > 0);
        assert_eq!(diagnostics.cpu_residual_batches, 0);
    } else {
        assert_eq!(diagnostics.cpu_authoring_probes, 0);
        assert_eq!(diagnostics.cpu_residual_batches, 0);
    }
}

#[test]
#[ignore = "requires CUDA driver and NVRTC; run on each training GPU"]
fn cuda_projection_repairs_pinned_meets_and_obeys_the_original_gate_and_bounds() {
    corrected_fixture(false);
}

#[test]
#[ignore = "requires CUDA driver and NVRTC; run on each training GPU"]
fn cuda_projection_repairs_named_meets_without_pinning_the_derived_mast() {
    corrected_fixture(true);
}

#[test]
#[ignore = "requires CUDA driver and NVRTC; run on each training GPU"]
fn latched_cuda_failure_stops_projection_before_gate_or_cpu_retry() {
    let design = octahedron(false);
    let backend = GpuGeometry::new_cuda(0).expect("CUDA and NVRTC");
    let (projection, _, _, bounds) = prepared(&design, backend.clone());
    backend.fail("injected CUDA launch failure".into());
    assert_eq!(
        backend.check().expect_err("fatal execution error"),
        "injected CUDA launch failure"
    );
    assert!(
        projection.project(&design, &bounds).is_none(),
        "even a feasible candidate cannot hide the execution error"
    );
    let diagnostics = backend.diagnostics();
    assert_eq!(diagnostics.cuda_failures, 1);
    assert_eq!(diagnostics.cuda_batches, 0);
    assert_eq!(diagnostics.cpu_residual_batches, 0);
    assert_eq!(diagnostics.cpu_projection_fallbacks, 0);
    assert_eq!(diagnostics.gate_refusals, 0);
}
