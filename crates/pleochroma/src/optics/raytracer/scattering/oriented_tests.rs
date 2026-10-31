//! Tests of the oriented-inclusion estimator: directional extinction, the free-path statistics,
//! the weights, and the agreement of the mixture sampler with the evaluated mixture phase.

use super::{
    super::NUM_CHANNELS,
    hg::henyey_greenstein_phase,
    oriented::{COMPONENTS, OrientedMedium, maybe_scatter_oriented, mis_phase_carry},
};
use crate::optics::{
    inclusions::{FamilyKind, InclusionSet, ResolvedFamily},
    polarization::StokesVector,
};
use glam::Vec3;
use std::f32::consts::{FRAC_PI_4, PI};

const LAMBDAS: [f32; NUM_CHANNELS] = [440.0, 480.0, 520.0, 550.0, 580.0, 620.0, 660.0, 700.0];
const HERO: usize = 3;

fn unit_vec(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z).normalize()
}

fn family(kind: FamilyKind, dir: Vec3, sigma: f32, p: u8, lobe: f32, rho: f32) -> ResolvedFamily {
    ResolvedFamily {
        kind,
        dir,
        sigma,
        p,
        lobe,
        rho,
    }
}

fn medium(set: &InclusionSet, sigma_hg: f32, g: f32) -> OrientedMedium<'_> {
    OrientedMedium {
        set,
        sigma_hg_model: sigma_hg,
        g,
        lambdas: &LAMBDAS,
        hero_idx: HERO,
        path_scale: 1.0,
    }
}

fn fresh_state() -> ([StokesVector; NUM_CHANNELS], [f32; NUM_CHANNELS]) {
    (
        [StokesVector::unpolarized(1.0); NUM_CHANNELS],
        [1.0; NUM_CHANNELS],
    )
}

/// Needles extinguish nothing along their axis and `sigma` across it; platelets the reverse;
/// random needles `pi / 4` of `sigma` from every side.
#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn directional_extinction_follows_the_family_shape() {
    let set = InclusionSet::new(&[
        family(FamilyKind::Needle, Vec3::Y, 0.4, 0, 0.1, 0.0),
        family(FamilyKind::Platelet, Vec3::X, 0.3, 0, 0.2, 0.0),
        family(FamilyKind::RandomNeedle, Vec3::ZERO, 0.2, 0, 0.1, 0.0),
    ]);
    let m = medium(&set, 0.0, 0.0);
    let along_y = m.extinctions(Vec3::Y, 550.0);
    let along_x = m.extinctions(Vec3::X, 550.0);
    assert!(along_y[1].abs() < 1e-6, "needle along its axis");
    assert!((along_x[1] - 0.4).abs() < 1e-6, "needle across its axis");
    assert!(along_y[2].abs() < 1e-6, "platelet edge on");
    assert!((along_x[2] - 0.3).abs() < 1e-6, "platelet face on");
    assert!((along_y[3] - 0.2 * FRAC_PI_4).abs() < 1e-6);
    assert!((along_x[3] - 0.2 * FRAC_PI_4).abs() < 1e-6);
    assert!(along_x[0].abs() < 1e-12, "no haze");
}

/// The exponent makes blue light extinguish more.
#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn a_blue_family_extinguishes_blue_light_more() {
    let set = InclusionSet::new(&[family(FamilyKind::Needle, Vec3::Y, 0.4, 2, 0.1, 0.0)]);
    let m = medium(&set, 0.0, 0.0);
    let blue = m.sigma_s(Vec3::X, 440.0);
    let red = m.sigma_s(Vec3::X, 700.0);
    assert!((blue - 0.4 * (550.0f32 / 440.0).powi(2)).abs() < 1e-5);
    assert!(blue > red);
}

/// With no absorption the chance of an event on a segment is `1 - exp(-sigma_s * length)`:
/// across the needles the mean free path is `1 / sigma`; along them nothing happens.
#[test]
fn the_free_path_across_needles_is_one_over_sigma_and_infinite_along_them() {
    let sigma = 0.8;
    let set = InclusionSet::new(&[family(FamilyKind::Needle, Vec3::Y, sigma, 0, 0.1, 0.0)]);
    let m = medium(&set, 0.0, 0.0);
    let alphas = [0.0f32; NUM_CHANNELS];
    let trials = 100_000u32;
    let count = |dir: Vec3, hit_t: f32| {
        (0..trials)
            .filter(|i| {
                let (mut stokes, mut path_pdf) = fresh_state();
                maybe_scatter_oriented(
                    &m,
                    &alphas,
                    dir,
                    hit_t,
                    i.wrapping_mul(0x9E37_79B9).wrapping_add(17),
                    0,
                    &mut stokes,
                    &mut path_pdf,
                )
                .is_some()
            })
            .count()
    };
    let expected = 1.0 - (-1.0f64).exp();
    let across = count(Vec3::X, 1.0 / sigma) as f64 / f64::from(trials);
    assert!((across - expected).abs() < 0.01, "{across} vs {expected}");
    assert_eq!(count(Vec3::Y, 50.0), 0, "never along the axis");
}

/// A grey family with no absorption and no haze conserves energy exactly: every event and every
/// survival carries weight one, and the phase ratio of the channels is one.
#[test]
fn a_grey_family_without_absorption_conserves_the_weight() {
    let set = InclusionSet::new(&[family(
        FamilyKind::Needle,
        unit_vec(0.2, 1.0, 0.1),
        0.9,
        0,
        0.15,
        0.4,
    )]);
    let m = medium(&set, 0.0, 0.0);
    let alphas = [0.0f32; NUM_CHANNELS];
    let dir = unit_vec(1.0, 0.3, -0.2);
    let (mut events, mut survivals) = (0, 0);
    for i in 0..4000u32 {
        let (mut stokes, mut path_pdf) = fresh_state();
        let seed = i.wrapping_mul(0x85EB_CA6B).wrapping_add(3);
        if let Some(scatter) =
            maybe_scatter_oriented(&m, &alphas, dir, 1.5, seed, 1, &mut stokes, &mut path_pdf)
        {
            events += 1;
            for (stokes, ratio) in stokes.iter().zip(scatter.phase_ratio) {
                assert!((stokes.intensity() - 1.0).abs() < 1e-4, "event weight");
                assert!((ratio - 1.0).abs() < 1e-4, "phase ratio");
            }
            assert!((scatter.new_dir.length() - 1.0).abs() < 1e-4);
        } else {
            survivals += 1;
            for stokes in &stokes {
                assert!((stokes.intensity() - 1.0).abs() < 1e-4, "survival weight");
            }
        }
    }
    assert!(
        events > 100 && survivals > 100,
        "{events} events, {survivals} survivals"
    );
}

/// Plain haze alone through the mixture is Henyey-Greenstein.
#[test]
fn the_haze_component_is_henyey_greenstein() {
    let set = InclusionSet::new(&[family(FamilyKind::Needle, Vec3::Y, 1e-12, 0, 0.1, 0.0)]);
    let m = medium(&set, 0.5, 0.4);
    let w_i = unit_vec(1.0, 0.0, 0.2);
    let w_o = unit_vec(0.5, 0.5, 0.2);
    let mixed = m.phase(550.0, w_i, w_o);
    let hg = henyey_greenstein_phase(w_i.dot(w_o), 0.4);
    assert!((mixed - hg).abs() < 1e-3 * hg, "{mixed} vs {hg}");
}

/// The sphere integral of `f` around `axis` (grid in `(s, phi)` with `s = 1 - 2 r^2`).
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn sphere_integral(axis: Vec3, f: impl Fn(Vec3) -> f64, n_r: usize, n_phi: usize) -> f64 {
    let (e1, e2) = crate::optics::inclusions::perpendicular_basis(axis);
    let mut total = 0.0f64;
    for i in 0..n_r {
        let r = (i as f32 + 0.5) / n_r as f32;
        let s = 1.0 - 2.0 * r * r;
        let radial = (1.0 - s * s).max(0.0).sqrt();
        let weight = f64::from(4.0 * r / n_r as f32 * (2.0 * PI / n_phi as f32));
        for j in 0..n_phi {
            let (sin, cos) = ((j as f32 + 0.5) * 2.0 * PI / n_phi as f32).sin_cos();
            total += f(axis * s + (e1 * cos + e2 * sin) * radial) * weight;
        }
    }
    total
}

fn mixed_medium_set() -> InclusionSet {
    InclusionSet::new(&[
        family(
            FamilyKind::Needle,
            unit_vec(0.2, 1.0, 0.1),
            0.5,
            2,
            0.25,
            0.3,
        ),
        family(
            FamilyKind::Platelet,
            unit_vec(-0.3, 0.2, 1.0),
            0.4,
            1,
            0.3,
            0.0,
        ),
        family(FamilyKind::RandomNeedle, Vec3::ZERO, 0.3, 0, 0.25, 0.2),
    ])
}

/// Every channel's mixture phase integrates to one, wavelength weights and all.
#[test]
fn the_mixture_phase_of_every_channel_is_normalised() {
    let set = mixed_medium_set();
    let m = medium(&set, 0.3, 0.4);
    let w_i = unit_vec(0.7, 0.2, 0.5);
    for k in [0, HERO, NUM_CHANNELS - 1] {
        let integral = sphere_integral(w_i, |w| f64::from(m.channel_phases(w_i, w).1[k]), 500, 300);
        assert!((integral - 1.0).abs() < 3e-2, "channel {k}: {integral}");
    }
}

/// The sampler draws from the density the evaluator reports: the mean of a test function over
/// samples equals its integral against the evaluated mixture.
#[test]
fn the_mixture_sampler_agrees_with_the_evaluated_phase() {
    let set = mixed_medium_set();
    let m = medium(&set, 0.3, 0.4);
    let w_i = unit_vec(0.7, 0.2, 0.5);
    let a = unit_vec(0.4, 0.8, -0.3);
    let tests: [&dyn Fn(Vec3) -> f64; 2] = [&|w: Vec3| f64::from(w.dot(a)), &|w: Vec3| {
        f64::from(w.dot(a)).powi(2)
    }];
    let n = 400_000u32;
    let mut sums = [0.0f64; 2];
    for i in 0..n {
        let mix = |salt: u32| {
            let mut x = i.wrapping_mul(0x9E37_79B9) ^ salt;
            x ^= x >> 16;
            x = x.wrapping_mul(0x85EB_CA6B);
            x ^= x >> 13;
            x = x.wrapping_mul(0xC2B2_AE35);
            x ^= x >> 16;
            (x >> 8) as f32 / 16_777_216.0
        };
        let w = m.sample_direction(w_i, mix(1), mix(2), mix(3));
        for (sum, f) in sums.iter_mut().zip(&tests) {
            *sum += f(w);
        }
    }
    for (sum, f) in sums.iter().zip(&tests) {
        let sampled = sum / f64::from(n);
        let integrated = sphere_integral(
            w_i,
            |w| f(w) * f64::from(m.phase(LAMBDAS[HERO], w_i, w)),
            500,
            300,
        );
        assert!(
            (sampled - integrated).abs() < 0.02,
            "sampled {sampled} vs integrated {integrated}"
        );
    }
}

/// With a chromatic family beside the achromatic haze the channels' mixtures differ, so the
/// ratio leaves one for the off-hero channels and is exactly one for the hero.
#[test]
fn a_chromatic_family_beside_haze_gives_a_phase_ratio() {
    let set = InclusionSet::new(&[family(FamilyKind::Needle, Vec3::Y, 0.6, 2, 0.2, 0.3)]);
    let m = medium(&set, 0.4, 0.3);
    let alphas = [0.0f32; NUM_CHANNELS];
    let dir = unit_vec(1.0, 0.2, 0.1);
    let mut seen_off_hero_ratio = false;
    for i in 0..400u32 {
        let (mut stokes, mut path_pdf) = fresh_state();
        let seed = i.wrapping_mul(0xC2B2_AE35).wrapping_add(11);
        if let Some(scatter) =
            maybe_scatter_oriented(&m, &alphas, dir, 3.0, seed, 2, &mut stokes, &mut path_pdf)
        {
            assert!((scatter.phase_ratio[HERO] - 1.0).abs() < 1e-5);
            assert!(scatter.hero_phase > 0.0);
            seen_off_hero_ratio |= (scatter.phase_ratio[0] - 1.0).abs() > 1e-3;
        }
    }
    assert!(seen_off_hero_ratio);
    assert_eq!(COMPONENTS, 5);
}

/// An ideal cone (needle lobe 0) is floored to a small positive lobe by `InclusionSet`, so the
/// phase density at a sampled direction is positive and can serve as the MIS partner. Before the
/// floor the sampled direction lay exactly on the cone, where `needle_pdf` returns 0.
#[test]
fn an_ideal_cone_lobe_is_floored_and_keeps_a_positive_phase() {
    let set = InclusionSet::new(&[family(FamilyKind::Needle, Vec3::Y, 0.8, 0, 0.0, 0.0)]);
    assert!(set.families()[0].lobe >= crate::optics::inclusions::NEEDLE_LOBE_FLOOR);
    let m = medium(&set, 0.0, 0.0);
    let alphas = [0.0f32; NUM_CHANNELS];
    let dir = unit_vec(1.0, 0.3, 0.1);
    let mut events = 0;
    for i in 0..200u32 {
        let (mut stokes, mut path_pdf) = fresh_state();
        let seed = i.wrapping_mul(0x9E37_79B9).wrapping_add(5);
        if let Some(scatter) =
            maybe_scatter_oriented(&m, &alphas, dir, 3.0, seed, 1, &mut stokes, &mut path_pdf)
        {
            events += 1;
            assert!(scatter.hero_phase > 0.0, "phase {}", scatter.hero_phase);
        }
    }
    assert!(events > 50, "{events} events");
}

/// A scatter whose hero phase is not positive carries no MIS partner (weight 1 at the escape, as
/// on the GPU); a positive one carries its density and direction.
#[test]
fn a_zero_phase_scatter_carries_no_mis_partner() {
    let dir = Vec3::Z;
    assert_eq!(mis_phase_carry(0.0, dir), None);
    assert_eq!(mis_phase_carry(-1.0, dir), None);
    assert_eq!(mis_phase_carry(0.3, dir), Some((0.3, dir)));
}
