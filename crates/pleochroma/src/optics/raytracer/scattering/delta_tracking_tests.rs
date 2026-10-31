//! Tests of the spectral delta tracking through a Bragg structure: unbiasedness against analytic
//! transmittance (homogeneous and heterogeneous), its composition with the homogeneous
//! extinction, the exact mirror direction, and bit-identity with the plain estimator when no
//! domain reflects.

use super::{
    super::{NUM_CHANNELS, camera::Ray},
    delta_tracking::{BraggEvent, TrackSegment, maybe_scatter_oriented_bragg, track_bragg},
    oriented::{OrientedMedium, maybe_scatter_oriented},
};
use crate::optics::{
    inclusions::{
        InclusionSet,
        bragg::BraggStructure,
        domains::{domain_at, seed_point},
    },
    polarization::StokesVector,
};
use glam::Vec3;

const LAMBDAS: [f32; NUM_CHANNELS] = [470.0, 500.0, 530.0, 550.0, 580.0, 610.0, 640.0, 680.0];
const HERO: usize = 3;

fn fresh_state() -> ([StokesVector; NUM_CHANNELS], [f32; NUM_CHANNELS]) {
    (
        [StokesVector::unpolarized(1.0); NUM_CHANNELS],
        [1.0; NUM_CHANNELS],
    )
}

fn medium<'a>(set: &'a InclusionSet, lambdas: &'a [f32; NUM_CHANNELS]) -> OrientedMedium<'a> {
    OrientedMedium {
        set,
        sigma_hg_model: 0.0,
        g: 0.0,
        lambdas,
        hero_idx: HERO,
        path_scale: 1.0,
    }
}

/// Every domain identical: one normal, one Bragg wavelength, all active. The medium is then
/// homogeneous whatever the domain ids are.
fn uniform_structure(
    normal: Vec3,
    lambda_nm: f32,
    cell: f32,
    r0: f32,
    mosaic: f32,
) -> BraggStructure {
    BraggStructure::new(
        7,
        cell,
        (lambda_nm, lambda_nm),
        0.06,
        1.0,
        r0,
        mosaic,
        normal,
    )
}

/// An opal-like structure: many small domains with random normals and colors.
fn varied_structure() -> BraggStructure {
    // A wide band, a strong peak and a mosaic spread, so a segment through random domain
    // normals crosses reflecting domains in several channels.
    BraggStructure::new(11, 0.25, (450.0, 650.0), 0.3, 1.0, 0.95, 0.3, Vec3::ZERO)
}

fn seeds(i: u32) -> u32 {
    i.wrapping_mul(0x9E37_79B9).wrapping_add(0x1234_5677)
}

/// The mean and the standard error of the mean of `n` samples from their sum and sum of squares.
#[expect(
    clippy::suboptimal_flops,
    clippy::suspicious_operation_groupings,
    reason = "test arithmetic written as the formula"
)]
fn mean_and_se(sum: f64, sum_sq: f64, n: f64) -> (f64, f64) {
    let mean = sum / n;
    let var = (sum_sq / n - mean * mean).max(0.0);
    (mean, (var / n).sqrt())
}

/// The exact optical depth `integral sigma_B,k ds` along a segment, by fine midpoint sampling of
/// the domain map.
fn optical_depth(
    structure: &BraggStructure,
    origin: Vec3,
    dir: Vec3,
    length: f32,
) -> [f64; NUM_CHANNELS] {
    let steps = 40_000u32;
    let ds = length / steps as f32;
    let mut tau = [0.0f64; NUM_CHANNELS];
    for i in 0..steps {
        let p = origin + dir * ((i as f32 + 0.5) * ds);
        let domain = domain_at(structure, p);
        let cos = dir.dot(domain.normal).abs();
        for k in 0..NUM_CHANNELS {
            tau[k] =
                f64::from(structure.sigma(&domain, cos, LAMBDAS[k])).mul_add(f64::from(ds), tau[k]);
        }
    }
    tau
}

/// The Monte Carlo transmittance of every channel through `length` of the medium `structure`
/// with the per-channel absorption `alphas`: the mean of the channel's Stokes intensity over
/// the paths that reach the end of the segment, with the standard error.
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn transmittance(
    structure: &BraggStructure,
    origin: Vec3,
    dir: Vec3,
    length: f32,
    alphas: &[f32; NUM_CHANNELS],
    trials: u32,
) -> [(f64, f64); NUM_CHANNELS] {
    let set = InclusionSet::EMPTY.with_structure(*structure);
    let m = medium(&set, &LAMBDAS);
    let mut sum = [0.0f64; NUM_CHANNELS];
    let mut sum_sq = [0.0f64; NUM_CHANNELS];
    for i in 0..trials {
        let (mut stokes, mut path_pdf) = fresh_state();
        let event = maybe_scatter_oriented_bragg(
            &m,
            structure,
            alphas,
            Ray { origin, dir },
            length,
            seeds(i),
            1,
            &mut stokes,
            &mut path_pdf,
        );
        if event.is_none() {
            for k in 0..NUM_CHANNELS {
                let w = f64::from(stokes[k].intensity());
                sum[k] += w;
                sum_sq[k] += w * w;
            }
        }
    }
    std::array::from_fn(|k| mean_and_se(sum[k], sum_sq[k], f64::from(trials)))
}

#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn assert_matches(estimate: (f64, f64), exact: f64, what: &str) {
    let (mean, se) = estimate;
    assert!(
        (mean - exact).abs() <= 4.0 * se + 2e-3,
        "{what}: estimated {mean} (se {se}), exact {exact}"
    );
}

/// A homogeneous Bragg medium transmits `exp(-sigma_B(lambda) L)` at every wavelength of the comb
/// at once, though only the hero drives the path.
#[test]
fn a_uniform_structure_transmits_the_analytic_transmittance_at_every_wavelength() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let structure = uniform_structure(normal, 560.0, 0.5, 0.9, 0.0);
    // 30 degrees to the normal: lambda_B = 560 cos(30) = 485 nm.
    let dir = Vec3::new(0.5, 0.0, 0.866_025_4).normalize();
    let length = 1.0;
    let estimates = transmittance(
        &structure,
        Vec3::ZERO,
        dir,
        length,
        &[0.0; NUM_CHANNELS],
        150_000,
    );
    let domain = domain_at(&structure, Vec3::ZERO);
    let cos = dir.dot(normal);
    let mut reflected = 0;
    for k in 0..NUM_CHANNELS {
        let sigma = structure.sigma(&domain, cos, LAMBDAS[k]);
        let exact = (-f64::from(sigma) * f64::from(length)).exp();
        assert_matches(estimates[k], exact, &format!("lambda {}", LAMBDAS[k]));
        if sigma > 0.05 {
            reflected += 1;
        }
    }
    assert!(
        reflected >= 2,
        "the comb must straddle the band for the check to mean anything"
    );
}

/// The hero reflects with the band's own probability, and the other channels weigh in so that
/// their transmittance is right even though the hero's path decided the event.
#[test]
fn an_off_band_hero_still_transmits_the_other_channels_correctly() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    // The band sits at the red end; the hero (550 nm) is far from it.
    let structure = uniform_structure(normal, 700.0, 0.5, 0.9, 0.0);
    let dir = normal;
    let estimates = transmittance(
        &structure,
        Vec3::ZERO,
        dir,
        1.0,
        &[0.0; NUM_CHANNELS],
        50_000,
    );
    let domain = domain_at(&structure, Vec3::ZERO);
    for k in 0..NUM_CHANNELS {
        let sigma = structure.sigma(&domain, 1.0, LAMBDAS[k]);
        assert_matches(estimates[k], (-f64::from(sigma)).exp(), "off-band hero");
    }
}

/// Many domains with random normals and colors: the heterogeneous transmittance equals
/// `exp(-tau)` for the optical depth integrated along the segment through the domain map.
#[test]
fn a_heterogeneous_structure_transmits_the_integrated_optical_depth() {
    let structure = varied_structure();
    let origin = Vec3::new(-0.2, 0.1, 0.05);
    let dir = Vec3::new(0.8, 0.3, 0.5).normalize();
    let length = 1.5;
    let tau = optical_depth(&structure, origin, dir, length);
    let estimates = transmittance(
        &structure,
        origin,
        dir,
        length,
        &[0.0; NUM_CHANNELS],
        150_000,
    );
    let mut informative = 0;
    for k in 0..NUM_CHANNELS {
        assert_matches(estimates[k], (-tau[k]).exp(), &format!("channel {k}"));
        if tau[k] > 0.1 {
            informative += 1;
        }
    }
    assert!(
        informative >= 2,
        "the segment must cross reflecting domains for the check to mean anything: {tau:?}"
    );
}

/// With absorption the homogeneous extinction composes with the Bragg reflection:
/// `T_k = exp(-(alpha_k + sigma_B,k) L)`.
#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn absorption_and_reflection_compose() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let structure = uniform_structure(normal, 560.0, 0.5, 0.8, 0.0);
    let dir = Vec3::new(0.3, 0.1, 0.9).normalize();
    let length = 0.8;
    let alphas: [f32; NUM_CHANNELS] = std::array::from_fn(|k| 0.2 + 0.15 * k as f32);
    let estimates = transmittance(&structure, Vec3::ZERO, dir, length, &alphas, 150_000);
    let domain = domain_at(&structure, Vec3::ZERO);
    let cos = dir.dot(normal).abs();
    for k in 0..NUM_CHANNELS {
        let sigma = structure.sigma(&domain, cos, LAMBDAS[k]);
        let exact = (-(f64::from(alphas[k]) + f64::from(sigma)) * f64::from(length)).exp();
        assert_matches(estimates[k], exact, &format!("channel {k}"));
    }
}

/// A reflection mirrors the ray in the lattice normal exactly when there is no mosaic spread,
/// at a place inside the segment, with all channels sharing the direction.
#[test]
fn a_reflection_is_the_exact_mirror_without_mosaic() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    // A band at normal incidence the whole comb sits in, and a thin dense structure: the first
    // tentative collision reflects.
    let structure = uniform_structure(normal, 550.0, 0.01, 0.95, 0.0);
    let set = InclusionSet::EMPTY.with_structure(structure);
    let lambdas = [550.0f32; NUM_CHANNELS];
    let m = medium(&set, &lambdas);
    let dir = Vec3::new(0.0, 0.0, 1.0);
    let mut reflected = 0;
    for i in 0..200u32 {
        let (mut stokes, mut path_pdf) = fresh_state();
        let found = maybe_scatter_oriented_bragg(
            &m,
            &structure,
            &[0.0; NUM_CHANNELS],
            Ray {
                origin: Vec3::ZERO,
                dir,
            },
            1.0,
            seeds(i),
            2,
            &mut stokes,
            &mut path_pdf,
        )
        .expect("a thin dense structure reflects");
        assert!(found.bragg);
        assert!(found.scatter.t_free_model >= 0.0 && found.scatter.t_free_model < 1.0);
        let mirror = dir - 2.0 * dir.dot(normal) * normal;
        assert!((found.scatter.new_dir - mirror).length() < 1e-5);
        assert!(found.scatter.phase_ratio.iter().all(|r| *r == 1.0));
        // One comb of one wavelength: every channel weighs like the hero.
        for s in &stokes {
            assert!((s.intensity() - 1.0).abs() < 1e-5);
        }
        reflected += 1;
    }
    assert_eq!(reflected, 200);
}

/// The mosaic spread turns the mirror direction, normalised and about the exact mirror.
#[test]
fn the_mosaic_spread_scatters_about_the_mirror_direction() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let structure = uniform_structure(normal, 550.0, 0.01, 0.95, 0.1);
    let set = InclusionSet::EMPTY.with_structure(structure);
    let lambdas = [550.0f32; NUM_CHANNELS];
    let m = medium(&set, &lambdas);
    let dir = Vec3::new(0.0, 0.0, 1.0);
    let mirror = -dir;
    let mut mean_cos = 0.0f64;
    let trials = 400u32;
    for i in 0..trials {
        let (mut stokes, mut path_pdf) = fresh_state();
        let found = maybe_scatter_oriented_bragg(
            &m,
            &structure,
            &[0.0; NUM_CHANNELS],
            Ray {
                origin: Vec3::ZERO,
                dir,
            },
            1.0,
            seeds(i),
            3,
            &mut stokes,
            &mut path_pdf,
        )
        .expect("reflects");
        let d = found.scatter.new_dir;
        assert!((d.length() - 1.0).abs() < 1e-4);
        mean_cos += f64::from(d.dot(mirror));
    }
    mean_cos /= f64::from(trials);
    assert!(
        mean_cos > 0.9 && mean_cos < 0.9999,
        "a small spread about the mirror direction: mean cosine {mean_cos}"
    );
}

/// Where no domain reflects (coverage zero), the wrapper is the plain estimator bit for bit.
#[test]
fn an_inactive_structure_is_bit_identical_to_the_plain_estimator() {
    let structure = BraggStructure::new(5, 0.2, (500.0, 600.0), 0.05, 0.0, 0.5, 0.0, Vec3::ZERO);
    let set = InclusionSet::EMPTY.with_structure(structure);
    let m = OrientedMedium {
        set: &set,
        sigma_hg_model: 0.4,
        g: 0.3,
        lambdas: &LAMBDAS,
        hero_idx: HERO,
        path_scale: 1.0,
    };
    let alphas: [f32; NUM_CHANNELS] = std::array::from_fn(|k| 0.05 * k as f32);
    let dir = Vec3::new(0.2, -0.9, 0.3).normalize();
    for i in 0..500u32 {
        let (mut stokes_a, mut pdf_a) = fresh_state();
        let (mut stokes_b, mut pdf_b) = fresh_state();
        let plain = maybe_scatter_oriented(
            &m,
            &alphas,
            dir,
            1.2,
            seeds(i),
            1,
            &mut stokes_a,
            &mut pdf_a,
        );
        let tracked = maybe_scatter_oriented_bragg(
            &m,
            &structure,
            &alphas,
            Ray {
                origin: Vec3::new(0.1, 0.2, 0.3),
                dir,
            },
            1.2,
            seeds(i),
            1,
            &mut stokes_b,
            &mut pdf_b,
        );
        assert_eq!(plain.is_some(), tracked.is_some());
        if let (Some(a), Some(b)) = (&plain, &tracked) {
            assert!(!b.bragg);
            assert_eq!(a.t_free_model.to_bits(), b.scatter.t_free_model.to_bits());
            assert_eq!(a.new_dir, b.scatter.new_dir);
        }
        for k in 0..NUM_CHANNELS {
            assert_eq!(
                stokes_a[k].intensity().to_bits(),
                stokes_b[k].intensity().to_bits()
            );
            assert_eq!(pdf_a[k].to_bits(), pdf_b[k].to_bits());
        }
    }
}

/// The same inputs give the same result, and the hero's null weight is exactly one.
#[test]
fn tracking_is_deterministic_and_the_hero_weighs_one() {
    // Half the domains reflect, and weaker than `varied_structure`'s, so the segments both
    // reflect and pass (with full coverage every segment reflects).
    let structure = BraggStructure::new(11, 0.25, (450.0, 650.0), 0.3, 0.5, 0.6, 0.3, Vec3::ZERO);
    let segment = TrackSegment {
        origin: Vec3::new(0.3, -0.2, 0.1),
        dir: Vec3::new(0.1, 0.9, 0.4).normalize(),
        t_max_model: 2.0,
        lambdas: &LAMBDAS,
        hero_idx: HERO,
        rng_seed: 99,
        bounce: 4,
    };
    let (mut events, mut passes) = (0, 0);
    for i in 0..300u32 {
        let seg = TrackSegment {
            rng_seed: seeds(i),
            ..segment
        };
        let a = track_bragg(&structure, &seg);
        let b = track_bragg(&structure, &seg);
        assert_eq!(a, b);
        assert_eq!(a.null_weight[HERO], 1.0);
        assert!(a.null_weight.iter().all(|w| w.is_finite() && *w >= 0.0));
        match a.event {
            BraggEvent::Reflect { t_model, .. } => {
                events += 1;
                assert!((0.0..2.0).contains(&t_model));
            }
            BraggEvent::Pass => passes += 1,
        }
    }
    assert!(
        events > 0 && passes > 0,
        "{events} reflections, {passes} passes"
    );
}

/// A reflecting domain is found where the map says it is: the segment through the seed point of
/// a domain reflects inside that domain's cell.
#[test]
fn a_reflection_happens_inside_the_domain_that_reflects() {
    let structure = varied_structure();
    let origin = seed_point(&structure, [0, 0, 0]) - Vec3::new(0.05, 0.0, 0.0);
    let dir = Vec3::X;
    for i in 0..100u32 {
        let seg = TrackSegment {
            origin,
            dir,
            t_max_model: 1.0,
            lambdas: &LAMBDAS,
            hero_idx: HERO,
            rng_seed: seeds(i),
            bounce: 1,
        };
        if let BraggEvent::Reflect { t_model, domain } = track_bragg(&structure, &seg).event {
            let at = domain_at(&structure, origin + dir * t_model);
            assert_eq!(at.id, domain.id);
            assert!(domain.active);
        }
    }
}
