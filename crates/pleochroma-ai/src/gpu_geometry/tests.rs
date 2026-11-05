use super::network::{Network, surfaces};
use pleochroma::geometry::cuts::StandardGemCuts;
use pleochroma_cut_core::Design;

fn pinned_design() -> Design {
    let mut design = Design::concave_fixture();
    design.concave_tiers.clear();
    design.concave_tier_ids.clear();
    design
}

#[test]
fn pinned_plane_shortcut_keeps_exact_production_provenance() {
    let design = pinned_design();
    let solved = design.solve().expect("fixture solve");
    let planes = design.planes_from_solved(&solved);
    let mut reference = design.preform.planes_offset(design.preform_y_offset);
    let offset = reference.len();
    for aliases in
        StandardGemCuts::asc_plane_provenance(&design.to_asc_schedule_from_solved(&solved))
    {
        for plane in aliases {
            reference.push(planes[offset + plane]);
        }
    }
    assert_eq!(surfaces(&design).expect("pinned fixture"), reference);
    let network = Network::capture(&design).expect("closed fixture");
    let residual = network.residuals(&reference).expect("finite rows");
    assert_eq!(residual.len(), network.rows);
    assert!(residual.iter().all(|r| r.is_finite()));
    assert_eq!(network.turn_travel(&design, &design), 0.0);
}

#[test]
fn unsupported_concave_program_has_an_explicit_fallback() {
    assert!(surfaces(&Design::concave_fixture()).is_none());
}

#[test]
fn authored_named_meets_keep_the_checked_cpu_solve_and_production_planes() {
    use pleochroma::geometry::meet_solver::{MeetConstraint, SolveControl};
    let mut design = pinned_design();
    design.tiers[2].constraint = MeetConstraint::MeetNamed(vec![design.tiers[1].name.clone()]);
    let solved = design
        .solve_with(&SolveControl::default())
        .expect("authored named meet");
    let planes = design.planes_from_solved(&solved);
    let mut expected = design.preform.planes_offset(design.preform_y_offset);
    let offset = expected.len();
    for aliases in
        StandardGemCuts::asc_plane_provenance(&design.to_asc_schedule_from_solved(&solved))
    {
        for plane in aliases {
            expected.push(planes[offset + plane]);
        }
    }
    assert!(!super::network::direct_masts(&design));
    assert_eq!(surfaces(&design).expect("exact authored planes"), expected);
}

#[cfg(not(feature = "cuda"))]
#[test]
fn cached_projection_reference_returns_the_core_certified_bounded_candidate() {
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_cut_core::{
        OptimizeOptions,
        design::{FacetSide, mast_through, tier_hinges},
        meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{Arc, Mutex},
    };
    let design = pinned_design();
    let baseline = Arc::new(MeetPreservationBaseline::capture(&design).expect("baseline"));
    let hinges = tier_hinges(&design, &design.solve().expect("solve"));
    let options = OptimizeOptions {
        vary_anchored: true,
        preservation_baseline: Some(baseline.clone()),
        anchor_hinges: hinges.iter().map(|(&i, h)| (i, h.point)).collect(),
        ..OptimizeOptions::default()
    };
    let free = pleochroma_cut_core::free_tier_indices_with(&design, &options);
    let anchors: BTreeSet<_> = free
        .iter()
        .copied()
        .filter(|i| hinges.contains_key(i))
        .collect();
    let backend = Arc::new(super::GpuGeometry {
        counters: super::Counters::default(),
        failure: Mutex::new(None),
        unsupported: Mutex::new(BTreeMap::new()),
        solid: None,
    });
    let projection = super::Projection::prepare(
        &design,
        &free,
        &anchors,
        true,
        None,
        baseline.clone(),
        PreservationPolicy::OPTIMIZE,
        backend,
        None,
    )
    .expect("supported network");
    let mut candidate = design.clone();
    let angle = candidate.tiers[2].angle_deg + 0.05;
    let hinge = hinges[&2];
    candidate.tiers[2].angle_deg = angle;
    candidate.tiers[2].constraint = MeetConstraint::ScaleReference(mast_through(
        angle,
        hinge.azimuth_rad,
        FacetSide::of_angle_deg(angle),
        hinge.point,
    ));
    let bounds = free
        .iter()
        .map(|&i| {
            (
                i,
                (
                    design.tiers[i].angle_deg - 0.1,
                    design.tiers[i].angle_deg + 0.1,
                ),
            )
        })
        .collect();
    let actual = projection
        .project(&candidate, &bounds)
        .expect("small bounded deformation");
    let expected = pleochroma_cut_core::optimize::project_preserved_design(
        &design,
        &candidate,
        &OptimizeOptions {
            angle_bounds: bounds,
            ..options
        },
        None,
    )
    .expect("CPU reference");
    assert_eq!(actual, expected);
    assert_ne!(actual, design);
    baseline
        .validate(&actual, PreservationPolicy::OPTIMIZE)
        .expect("authoritative final gate");
}

#[cfg(feature = "cuda")]
#[test]
#[ignore = "requires an available CUDA driver and NVRTC; run explicitly on the training GPU"]
fn cuda_f64_rows_match_cpu_for_batched_angle_and_depth_probes() {
    let design = pinned_design();
    let network = Network::capture(&design).expect("closed fixture");
    let runtime = super::cuda::Runtime::new(0).expect("CUDA initialized");
    let device = runtime.prepare(&network).expect("cached network uploaded");
    let mut batch = vec![surfaces(&design).expect("pinned")];
    for delta in [-0.05, -0.002, 0.002, 0.05] {
        let mut candidate = design.clone();
        candidate.tiers[2].angle_deg += delta;
        batch.push(surfaces(&candidate).expect("angle probe"));
    }
    let got = runtime
        .batch(&network, &device, &batch, None, false, false)
        .expect("batched residuals");
    for (planes, actual) in batch.iter().zip(got) {
        let expected = network.residuals(planes).expect("CPU rows");
        for (a, b) in actual
            .rows
            .expect("finite GPU residuals")
            .iter()
            .zip(expected)
        {
            assert!((a - b).abs() <= 2e-12 * (1.0 + b.abs()), "GPU {a}, CPU {b}");
        }
    }
}

fn production_provenance(design: &Design) -> Vec<super::network::Plane> {
    let solved = design.solve().expect("fixture solve");
    let production = design.planes_from_solved(&solved);
    let mut result = design.preform.planes_offset(design.preform_y_offset);
    let offset = result.len();
    for tier in StandardGemCuts::asc_plane_provenance(&design.to_asc_schedule_from_solved(&solved))
    {
        result.extend(tier.into_iter().map(|index| production[offset + index]));
    }
    result
}

#[test]
fn direct_planes_follow_changed_aliases_even_when_plane_count_is_unchanged() {
    use pleochroma::geometry::meet_solver::MeetConstraint;
    let mut design = pinned_design();
    let mut tier = design.tiers[0].clone();
    tier.indices = vec![0.0];
    tier.angle_deg = 45.0;
    tier.constraint = MeetConstraint::ScaleReference(1.0);
    design.tiers = vec![tier.clone(), tier.clone(), tier];
    design.tiers[2].constraint = MeetConstraint::ScaleReference(1.01);
    let first = production_provenance(&design);
    let first_count = design.planes_from_solved(&design.solve().unwrap()).len();
    assert_eq!(super::network::direct_surfaces(&design).unwrap(), first);
    design.tiers[1].constraint = MeetConstraint::ScaleReference(1.01);
    let second = production_provenance(&design);
    assert_eq!(
        design.planes_from_solved(&design.solve().unwrap()).len(),
        first_count
    );
    assert_ne!(first, second);
    assert_eq!(super::network::direct_surfaces(&design).unwrap(), second);
}

#[test]
fn direct_planes_match_production_for_flat_aliases_gear_and_cheaters() {
    use pleochroma::geometry::meet_solver::MeetConstraint;
    let mut design = pinned_design();
    let mut flat = design.tiers[0].clone();
    flat.angle_deg = 0.0;
    flat.indices = vec![0.0, 24.0, 48.0];
    flat.constraint = MeetConstraint::ScaleReference(1.0);
    let mut facet = flat.clone();
    facet.angle_deg = -42.0;
    facet.indices = vec![0.0, 12.0, 24.0, 36.0];
    let mut implicit = facet.clone();
    implicit.indices.clear();
    design.tiers = vec![flat, facet.clone(), facet, implicit];
    design.meta.gear_teeth = -96;
    design.meta.gear_reference_angle = 3.5;
    design.meta.symmetry_order = 4;
    design.meta.mirror = true;
    design.cheater_offsets_deg.insert(1, 1.25);
    design.cheater_offsets_deg.insert(2, -2.0);
    assert_eq!(
        super::network::direct_surfaces(&design).unwrap(),
        production_provenance(&design)
    );
}

#[test]
fn direct_planes_match_checked_production_after_relation_updates() {
    use pleochroma_cut_core::design::{Expr, TierRelation};

    let mut design = pinned_design();
    design.tiers[0].angle_deg = 40.0;
    design.tiers[1].angle_deg = -35.0;
    design.tiers[2].angle_deg = -0.0;
    design.tiers[2].indices.clear();
    design.tiers[3].angle_deg = 0.0;
    design.tiers[3].indices.clear();
    design.meta.gear_teeth = -96;
    design.meta.gear_reference_angle = 2.5;
    let relation = TierRelation::new(Expr::Add(
        Box::new(Expr::Ref(design.tier_ids[0])),
        Box::new(Expr::Number(2.0)),
    ));
    design.tier_relations.insert(design.tier_ids[1], relation);

    for angle in [40.0, 41.25] {
        design.tiers[0].angle_deg = angle;
        for (index, related_angle) in design.evaluate_relations().unwrap() {
            design.tiers[index].angle_deg = related_angle;
        }
        assert_eq!(design.tiers[1].angle_deg, -(angle + 2.0));
        assert!(super::network::direct_masts(&design));
        assert_eq!(
            super::network::direct_surfaces(&design).unwrap(),
            production_provenance(&design)
        );
    }
}
