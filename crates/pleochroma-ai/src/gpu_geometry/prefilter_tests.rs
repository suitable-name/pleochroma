//! One-way safety contracts: a conservative GPU proof may avoid a CPU gate only
//! when the original production gate also refuses. All uncertain cases pass on.
use super::{
    cuda::Runtime,
    network::{Network, surfaces},
};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{
    Design,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
};

fn pinned() -> Design {
    let mut design = Design::concave_fixture();
    design.concave_tiers.clear();
    design.concave_tier_ids.clear();
    design
}

#[test]
#[ignore = "requires CUDA and NVRTC; exercises conservative gate boundary arithmetic"]
fn accepted_states_are_never_refuted_at_incidence_slack_and_gauge_boundaries() {
    let design = pinned();
    let baseline = MeetPreservationBaseline::capture(&design).expect("baseline");
    let network = Network::capture(&design).expect("canonical CPU reference network");
    let runtime = Runtime::new(0).expect("CUDA");
    let device = runtime.prepare(&network).expect("uploaded network");
    // Straddle the reproduction threshold, f32 plane quantization and both signs
    // of depth travel. Preserve imported-mast slack in the original network.
    let mut candidates = vec![design.clone()];
    for factor in [0.0, 0.25, 0.999_999, 1.0, 1.000_001, 2.0, 20.0] {
        for sign in [-1.0, 1.0] {
            let mut candidate = design.clone();
            if let MeetConstraint::ScaleReference(mast) = &mut candidate.tiers[2].constraint {
                *mast += sign * factor * 2e-6 * network.scale;
            }
            candidates.push(candidate);
        }
    }
    let mut impossible = design.clone();
    if let MeetConstraint::ScaleReference(mast) = &mut impossible.tiers[2].constraint {
        *mast += 2.0 * network.scale;
    }
    candidates.push(impossible);
    let planes = candidates
        .iter()
        .map(|d| surfaces(d).expect("finite planes"))
        .collect::<Vec<_>>();
    for policy in [
        PreservationPolicy::OPTIMIZE,
        PreservationPolicy::RETARGET,
        PreservationPolicy {
            fixed_outline: true,
            lock_girdle_gauge: false,
            ..PreservationPolicy::OPTIMIZE
        },
    ] {
        let limits = vec![policy.max_junction_displacement * network.scale; candidates.len()];
        let probes = runtime
            .batch(
                &network,
                &device,
                &planes,
                Some(&limits),
                policy.fixed_outline,
                policy.lock_girdle_gauge,
            )
            .expect("interval batch");
        assert!(
            probes[0].possible,
            "the unchanged baseline must survive every policy prefilter"
        );
        assert!(
            !probes.last().expect("impossible probe").possible,
            "an impossible support slab must skip the heavyweight gate"
        );
        for (candidate, probe) in candidates.iter().zip(probes) {
            if baseline.validate(candidate, policy).is_ok() {
                assert!(
                    probe.possible,
                    "GPU refuted a production-accepted boundary candidate"
                );
                let planes = surfaces(candidate).expect("production planes");
                assert_ne!(
                    runtime
                        .arrangement_possible(&network, &device, &planes)
                        .expect("arrangement proof"),
                    Some(false),
                    "triple proof refuted a production-accepted candidate"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires CUDA and NVRTC; exercises discrete arrangement rather than slab feasibility"]
fn triple_proof_finds_missing_point_even_when_slabs_overlap() {
    use glam::DVec3;
    let mut network = Network::capture(&pinned()).expect("network");
    network.junctions.truncate(1);
    let junction = &mut network.junctions[0];
    junction.point = DVec3::ZERO;
    junction.support = vec![0, 1, 2, 3];
    junction.basis = [0, 1, 2];
    junction.targets = vec![0.0];
    junction.bound = 0.05;
    network.surfaces = 4;
    network.rows = 4;
    network.scale = 1.0;
    network.gauge.clear();
    let runtime = Runtime::new(0).expect("CUDA");
    let device = runtime.prepare(&network).expect("upload");
    // The slabs intersect around (0.05, 0.05, 0), but the triple points
    // (0,0,0), (0.12,0,0) and (0,0.12,0) all miss a support.
    let planes = vec![
        (DVec3::X, 0.0),
        (DVec3::Y, 0.0),
        (DVec3::Z, 0.0),
        (DVec3::X + DVec3::Y, 0.12),
    ];
    let probe = runtime
        .batch(
            &network,
            &device,
            std::slice::from_ref(&planes),
            Some(&[1.0]),
            false,
            false,
        )
        .expect("slabs")
        .pop()
        .expect("probe");
    assert!(
        probe.possible,
        "the overlapping slabs alone cannot refute this point"
    );
    assert_eq!(
        runtime
            .arrangement_possible(&network, &device, &planes)
            .expect("proof"),
        Some(false)
    );
    let mut with_witness = planes;
    with_witness[3].1 = 0.04;
    assert_eq!(
        runtime
            .arrangement_possible(&network, &device, &with_witness)
            .expect("proof"),
        Some(true)
    );
}

#[test]
#[ignore = "requires CUDA and NVRTC; checks rounded production triple witnesses"]
fn production_simd_triple_points_remain_possible_at_their_exact_slack() {
    use glam::DVec3;
    use pleochroma::simd::{TripleBatch, solve_triple_batch};
    let runtime = Runtime::new(0).expect("CUDA");
    for delta in [1e-5, 1e-4, 0.01, 0.2, 1.0] {
        let mut network = Network::capture(&pinned()).expect("network");
        network.junctions.truncate(1);
        let a = (DVec3::X, 0.13);
        let b = (DVec3::new(1.0, delta, 0.0), 0.130_001);
        let c = (DVec3::Z, 0.17);
        let mut batch = TripleBatch::default();
        batch.push(a, b, c);
        let solved = solve_triple_batch(&batch);
        let point = DVec3::new(solved.vx[0], solved.vy[0], solved.vz[0]);
        let extra = DVec3::new(0.3, -0.4, 0.5);
        let planes = vec![a, b, c, (extra, extra.dot(point))];
        let slack = planes
            .iter()
            .map(|(n, d)| (n.dot(point) - d).abs())
            .fold(0.0_f64, f64::max);
        let junction = &mut network.junctions[0];
        junction.point = point;
        junction.support = vec![0, 1, 2, 3];
        junction.basis = [0, 1, 2];
        junction.targets = vec![0.0];
        junction.bound = slack;
        network.surfaces = 4;
        network.rows = 4;
        // Both the production normalizer and the proof use scale 1 here.
        network.scale = 1.0;
        network.gauge.clear();
        let device = runtime.prepare(&network).expect("upload");
        assert_ne!(
            runtime
                .arrangement_possible(&network, &device, &planes)
                .expect("proof"),
            Some(false),
            "the CPU SIMD point is a witness at delta {delta}"
        );
    }
}

#[test]
#[ignore = "requires CUDA and NVRTC; corpus regression for imported Stardust meets"]
fn stardust_production_accepted_probes_are_never_refuted() {
    for text in [
        include_str!("fixtures/stardust-41-076.psdd"),
        include_str!("fixtures/stardust-30-129.psdd"),
    ] {
        let design = pleochroma_cut_core::native::design_from_str(text)
            .expect("Stardust")
            .design;
        let baseline = MeetPreservationBaseline::capture(&design).expect("baseline");
        let network = Network::capture(&design).expect("production network");
        let runtime = Runtime::new(0).expect("CUDA");
        let device = runtime.prepare(&network).expect("upload");
        let mut candidates = vec![design.clone()];
        for tier in 0..design.tiers.len().min(8) {
            for delta in [-2e-6, 2e-6, 2e-5] {
                let mut candidate = design.clone();
                if let MeetConstraint::ScaleReference(mast) = &mut candidate.tiers[tier].constraint
                {
                    *mast += delta * network.scale;
                    candidates.push(candidate);
                }
            }
        }
        for candidate in candidates {
            if baseline
                .validate(&candidate, PreservationPolicy::OPTIMIZE)
                .is_ok()
            {
                let planes = surfaces(&candidate).expect("production planes");
                assert_ne!(
                    runtime
                        .arrangement_possible(&network, &device, &planes)
                        .expect("proof"),
                    Some(false)
                );
            }
        }
    }
}

#[test]
#[ignore = "requires CUDA and NVRTC; checks bounded proof interruption and work bypass"]
fn arrangement_checks_after_its_bounded_tile_and_bypasses_larger_work() {
    use super::cuda::{ArrangementOutcome, ArrangementWork};
    use glam::DVec3;
    use std::cell::Cell;

    let mut network = Network::capture(&pinned()).expect("network");
    network.junctions.truncate(1);
    network.junctions[0].support = vec![0, 1, 2];
    network.junctions[0].basis = [0, 1, 2];
    network.junctions[0].targets.clear();
    network.surfaces = 59;
    network.rows = 3;
    network.gauge.clear();
    let runtime = Runtime::new(0).expect("CUDA");
    let device = runtime.prepare(&network).expect("upload");
    let planes = vec![(DVec3::X, 0.0); 59];
    let calls = Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            Err("stopped".to_string())
        } else {
            Ok(())
        }
    };
    let mut work = ArrangementWork::default();
    assert!(
        runtime
            .arrangement_proof(&network, &device, &planes, &mut work, &check)
            .is_err()
    );
    assert_eq!(work.tiles, 1);
    assert_eq!(work.triples, 32_509);

    network.surfaces = 60;
    let mut budget_work = ArrangementWork::default();
    let over_budget = vec![(DVec3::X, 0.0); 60];
    let outcome = runtime
        .arrangement_proof(
            &network,
            &device,
            &over_budget,
            &mut budget_work,
            &|| Ok(()),
        )
        .expect("proof work bypass");
    assert!(matches!(outcome, ArrangementOutcome::Capacity));
    assert_eq!(budget_work.tiles, 0);
    assert_eq!(budget_work.triples, 0);

    // A single tile can still be too costly when each triple visits many supports.
    network.surfaces = 59;
    network.junctions = vec![network.junctions[0].clone(); 9];
    let mut support_work = ArrangementWork::default();
    let outcome = runtime
        .arrangement_proof(&network, &device, &planes, &mut support_work, &|| Ok(()))
        .expect("support-check work bypass");
    assert!(matches!(outcome, ArrangementOutcome::Capacity));
    assert_eq!(support_work.tiles, 0);
    assert_eq!(support_work.triples, 0);

    let mut bypass_work = ArrangementWork::default();
    let oversized = vec![(DVec3::X, 0.0); 257];
    let outcome = runtime
        .arrangement_proof(&network, &device, &oversized, &mut bypass_work, &|| Ok(()))
        .expect("capacity bypass");
    assert!(matches!(outcome, ArrangementOutcome::Capacity));
    assert_eq!(bypass_work.tiles, 0);
    assert_eq!(bypass_work.triples, 0);
}

#[test]
#[ignore = "requires CUDA and NVRTC; exercises interval uncertainty and imported slack"]
fn singular_or_large_roundoff_enclosures_are_unknown_instead_of_rejected() {
    let design = pinned();
    let mut network = Network::capture(&design).expect("network");
    // A deliberately singular *fixed basis* says nothing about the production
    // arrangement, which may use other supports. Its interval check must yield
    // UNKNOWN. Turning off gauge isolates that exact uncertainty contract.
    for junction in &mut network.junctions {
        junction.basis[2] = junction.basis[1];
    }
    let runtime = Runtime::new(0).expect("CUDA");
    let device = runtime.prepare(&network).expect("uploaded network");
    let planes = vec![surfaces(&design).expect("planes")];
    let limits = [0.5 * network.scale];
    let probe = runtime
        .batch(&network, &device, &planes, Some(&limits), false, false)
        .expect("singular interval probe")
        .pop()
        .expect("one probe");
    assert!(
        probe.possible,
        "singular inversion cannot be an impossibility proof"
    );
    assert!(
        probe.rows.is_none(),
        "singular numeric residuals stay unusable"
    );
    let probe = runtime
        .batch(
            &network,
            &device,
            &planes,
            Some(&[f64::INFINITY]),
            false,
            true,
        )
        .expect("unbounded enclosure")
        .pop()
        .expect("one probe");
    assert!(
        probe.possible,
        "non-finite enclosure must retain the authoritative CPU gate"
    );
}

#[test]
#[ignore = "requires CUDA and NVRTC; canonical capture must retain production ordering"]
fn gpu_canonical_capture_matches_the_cpu_supports_basis_and_quantized_targets() {
    let solid = crate::gpu_solid::GpuSolid::new_cuda(0).expect("CUDA solid");
    let mut large_rough = pinned();
    large_rough.preform = pleochroma_cut_core::preform::PreformSpec::block(512.0, 512.0, 512.0);
    for mut design in [pinned(), large_rough] {
        // Exact imported-style decimal depths cover nonzero row targets as well.
        for tier in &mut design.tiers {
            if let MeetConstraint::ScaleReference(mast) = &mut tier.constraint {
                *mast = (*mast * 100_000.0).round() / 100_000.0;
            }
        }
        let cpu = Network::capture(&design).expect("CPU capture");
        let gpu = Network::capture_with_solid(&design, Some(&solid))
            .expect("CUDA capture")
            .expect("finite stone");
        assert_eq!(cpu.scale.to_bits(), gpu.scale.to_bits());
        assert_eq!(cpu.junctions.len(), gpu.junctions.len());
        assert!(gpu.same_proof_metadata(&cpu));
        let mut altered = gpu.clone();
        altered.junctions[0].bound = f64::from_bits(altered.junctions[0].bound.to_bits() + 1);
        assert!(
            !altered.same_proof_metadata(&cpu),
            "one bit of slack disables cheap rejection"
        );
        for (a, b) in cpu.junctions.iter().zip(gpu.junctions) {
            assert_eq!(a.support, b.support);
            assert_eq!(a.basis, b.basis);
            assert_eq!(a.point, b.point);
            assert_eq!(a.targets, b.targets);
            assert_eq!(a.bound, b.bound);
        }
    }
}
