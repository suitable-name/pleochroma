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
            }
        }
    }
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
        for (a, b) in cpu.junctions.iter().zip(gpu.junctions) {
            assert_eq!(a.support, b.support);
            assert_eq!(a.basis, b.basis);
            assert_eq!(a.point, b.point);
            assert_eq!(a.targets, b.targets);
            assert_eq!(a.bound, b.bound);
        }
    }
}
