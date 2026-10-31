//! Tests of the GGX lobe, the total integrated scatter, the packed word and the sweep.

use std::f32::consts::{PI, TAU};

use glam::Vec3;

use super::{
    FacetSurface, FacetingMachine, GgxLobe, PolishPreset, SurfaceKind, SweepPreset,
    facet_surface::{DEFAULT_GROUND_ALPHA, effective_anisotropy, halo_alpha},
    micro::reflect_about,
    sweep::{scratch_angle_deg, scratch_direction},
    tis::{tis_reflection, tis_transmission},
};
use crate::optics::raytracer::hash_u32;

/// A deterministic uniform number in `[0, 1]` for the index `i` of the stream `salt`.
fn uniform(i: u32, salt: u32) -> f32 {
    hash_u32(i.wrapping_mul(0x9E37_79B9) ^ salt) as f32 / 4_294_967_295.0
}

/// The unit vector of polar angle `theta` and azimuth `phi`.
fn direction(theta: f32, phi: f32) -> Vec3 {
    let (st, ct) = theta.sin_cos();
    Vec3::new(st * phi.cos(), st * phi.sin(), ct)
}

/// The integral over the upper hemisphere of `f(m)` per unit solid angle, by the midpoint rule
/// in `(cos θ, φ)`.
fn integrate_hemisphere(f: impl Fn(Vec3) -> f32) -> f64 {
    let (n_cos, n_phi) = (2000usize, 360usize);
    let mut total = 0.0f64;
    for a in 0..n_cos {
        // Squared spacing resolves the narrow peak at the pole.
        let t = (a as f32 + 0.5) / n_cos as f32;
        let cos_t = t.mul_add(-t, 1.0);
        let weight = 2.0 * t / n_cos as f32;
        let theta = cos_t.clamp(-1.0, 1.0).acos();
        for b in 0..n_phi {
            let phi = (b as f32 + 0.5) / n_phi as f32 * TAU;
            total += f64::from(f(direction(theta, phi)) * weight * (TAU / n_phi as f32));
        }
    }
    total
}

const LOBES: [(f32, f32); 3] = [(0.3, 0.3), (0.5, 0.2), (0.12, 0.25)];

#[test]
fn the_normal_distribution_is_normalised_over_projected_area() {
    for (ax, ay) in LOBES {
        let lobe = GgxLobe {
            alpha_x: ax,
            alpha_y: ay,
        };
        let total = integrate_hemisphere(|m| lobe.d(m) * m.z);
        assert!((total - 1.0).abs() < 1e-2, "({ax}, {ay}): {total}");
    }
}

#[test]
fn the_visible_normal_density_integrates_to_one_and_g1_is_its_normaliser() {
    for (ax, ay) in LOBES {
        let lobe = GgxLobe {
            alpha_x: ax,
            alpha_y: ay,
        };
        for theta in [0.0f32, 0.5, 1.0, 1.35] {
            let wi = direction(theta, 0.7);
            let visible = integrate_hemisphere(|m| lobe.visible_density(wi, m));
            assert!(
                (visible - 1.0).abs() < 1e-2,
                "({ax}, {ay}) {theta}: {visible}"
            );
            let projected =
                integrate_hemisphere(|m| wi.dot(m).max(0.0) * lobe.d(m)) / f64::from(wi.z);
            // Heitz (2014): the projected area of the micro-normals seen from `wi`, over that of
            // the macro surface, is `1 / G1(wi)`.
            assert!(
                (projected - 1.0 / f64::from(lobe.g1(wi))).abs() < 1e-3,
                "({ax}, {ay}) {theta}: {projected} vs 1/{}",
                lobe.g1(wi)
            );
        }
    }
}

/// The sample histogram of `sample_visible` against the density it should follow.
#[test]
#[expect(
    clippy::suboptimal_flops,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test arithmetic written as the formula; histogram bins are small non-negative indices"
)]
fn the_visible_normal_sampler_follows_its_density() {
    let lobe = GgxLobe {
        alpha_x: 0.4,
        alpha_y: 0.2,
    };
    let wi = direction(0.9, 0.4);
    let (n_cos, n_phi) = (12usize, 10usize);
    let samples = 400_000u32;
    let mut counts = vec![0u32; n_cos * n_phi];
    for i in 0..samples {
        let m = lobe.sample_visible(wi, uniform(i, 1), uniform(i, 2));
        assert!(m.z >= 0.0 && (m.length() - 1.0).abs() < 1e-4 && wi.dot(m) > -1e-5);
        // Bins uniform in the squared polar angle's cosine complement and in the azimuth.
        let t = (1.0 - m.z).sqrt().min(0.999_999);
        let phi = m.y.atan2(m.x).rem_euclid(TAU);
        let a = (t * n_cos as f32) as usize;
        let b = ((phi / TAU) * n_phi as f32) as usize;
        counts[a.min(n_cos - 1) * n_phi + b.min(n_phi - 1)] += 1;
    }
    for a in 0..n_cos {
        for b in 0..n_phi {
            // The expected mass of the bin: a fine midpoint rule of the density over it.
            let mut expected = 0.0f64;
            let sub = 12usize;
            for sa in 0..sub {
                for sb in 0..sub {
                    let t = (a as f32 + (sa as f32 + 0.5) / sub as f32) / n_cos as f32;
                    let phi = (b as f32 + (sb as f32 + 0.5) / sub as f32) / n_phi as f32 * TAU;
                    let cos_t = 1.0 - t * t;
                    let m = direction(cos_t.clamp(-1.0, 1.0).acos(), phi);
                    let cell =
                        (2.0 * t / n_cos as f32 / sub as f32) * (TAU / n_phi as f32 / sub as f32);
                    expected += f64::from(lobe.visible_density(wi, m) * cell);
                }
            }
            let observed = f64::from(counts[a * n_phi + b]) / f64::from(samples);
            let sigma = (expected * (1.0 - expected) / f64::from(samples)).sqrt();
            assert!(
                (observed - expected).abs() < 5.0 * sigma + 2e-4,
                "bin ({a}, {b}): observed {observed}, expected {expected}"
            );
        }
    }
}

/// The reflected directions spread across the scratches by the ratio of the widths. The median
/// of the absolute component is used: the second moment of a GGX lobe has a logarithmic tail that
/// differs between the two axes.
#[test]
fn the_reflected_spread_follows_the_anisotropy_ratio() {
    let lobe = GgxLobe::anisotropic(0.03, 4.0);
    let wi = Vec3::Z;
    let (mut across, mut along) = (Vec::new(), Vec::new());
    for i in 0..100_000u32 {
        let m = lobe.sample_visible(wi, uniform(i, 11), uniform(i, 12));
        let o = reflect_about(wi, m);
        across.push(o.x.abs());
        along.push(o.y.abs());
    }
    let median = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let ratio = median(&mut across) / median(&mut along);
    assert!((ratio / 4.0 - 1.0).abs() < 0.05, "ratio {ratio}");
    assert!((lobe.alpha_x / lobe.alpha_y - 4.0).abs() < 1e-5);
    assert!((lobe.alpha_mean() - 0.03).abs() < 1e-6);
}

#[test]
fn the_scatter_fraction_follows_the_formula_and_its_limits() {
    for (cos_i, lambda) in [(1.0f32, 450.0f32), (0.7, 550.0), (0.3, 650.0)] {
        let x = (4.0 * PI * 1.0 * 2.5 * cos_i / lambda).powi(2);
        let tis = tis_reflection(2.5, 1.0, cos_i, lambda);
        // The specular fraction is exp(-x); to first order the scatter is x.
        assert!(((1.0 - tis) - (-x).exp()).abs() < 1e-6);
        assert!((tis - x).abs() < x * x);
    }
    assert_eq!(tis_reflection(0.0, 1.0, 1.0, 550.0), 0.0);
    assert_eq!(tis_transmission(0.0, 1.0, 1.77, 0.8, 550.0), 0.0);
    assert!(tis_reflection(500.0, 1.0, 1.0, 550.0) > 0.999);
    // Bluer light scatters more; a deeper roughness scatters more.
    assert!(tis_reflection(6.0, 1.0, 0.9, 450.0) > tis_reflection(6.0, 1.0, 0.9, 650.0));
    assert!(tis_reflection(25.0, 1.0, 0.9, 550.0) > tis_reflection(6.0, 1.0, 0.9, 550.0));
    // An index-matched interface does not scatter in transmission.
    assert!(tis_transmission(6.0, 1.5, 1.5, 0.8, 550.0) < 1e-9);
    // Past the critical angle the transmission value falls back to the reflection one.
    assert_eq!(
        tis_transmission(6.0, 1.77, 1.0, 0.2, 550.0),
        tis_reflection(6.0, 1.77, 0.2, 550.0)
    );
    // The presets, at 550 nm and normal incidence (R0 table, without the Fresnel factor).
    let mirror = tis_reflection(PolishPreset::Mirror.rq_nm(), 1.0, 1.0, 550.0);
    assert!((mirror - 5.2e-4).abs() < 5e-5, "{mirror}");
}

#[test]
fn the_packed_word_keeps_ideal_and_frosted_and_round_trips_the_fields() {
    assert_eq!(FacetSurface::IDEAL.packed(), 0);
    assert_eq!(FacetSurface::from_packed(0).kind(), SurfaceKind::Ideal);
    assert_eq!(FacetSurface::frosted().packed(), 1);
    let frosted = FacetSurface::from_packed(1);
    assert_eq!(frosted.kind(), SurfaceKind::Frosted);
    assert!(frosted.is_ground() && !frosted.has_roughness());
    assert_eq!(frosted.ground_alpha(), DEFAULT_GROUND_ALPHA);
    assert_eq!(frosted.anisotropy(), 1.0);
    assert!(frosted.custom_angle_deg().is_none() && !frosted.no_sweep());

    for preset in [
        PolishPreset::Mirror,
        PolishPreset::Fine,
        PolishPreset::Commercial,
        PolishPreset::Prepolish,
    ] {
        let surface = FacetSurface::from_preset(preset, 1.0);
        assert_eq!(surface.kind(), SurfaceKind::Polished);
        assert!((surface.rq_nm() - preset.rq_nm()).abs() < 1e-6);
        assert!((surface.anisotropy() - preset.anisotropy()).abs() < 1.0 / 16.0);
        // The halo width from the packed Rq is the preset's alpha.
        assert!(
            (halo_alpha(surface.rq_nm()) / preset.halo_alpha() - 1.0).abs() < 1e-3,
            "{preset:?}"
        );
        assert!(surface.has_roughness());
    }
    // Scratch Strength 0 is a round lobe; the default carries 30 % of the preset's ratio.
    assert_eq!(
        FacetSurface::from_preset(PolishPreset::Commercial, 0.0).anisotropy(),
        1.0
    );
    let default = FacetSurface::from_preset(PolishPreset::Commercial, 0.3);
    assert!((default.anisotropy() - effective_anisotropy(3.0, 0.3)).abs() < 1.0 / 16.0);

    let ground = FacetSurface::ground(0.3, 2.0)
        .with_custom_angle(90.0)
        .with_no_sweep();
    assert_eq!(ground.kind(), SurfaceKind::Ground);
    assert!((ground.ground_alpha() - 0.3).abs() < 1e-6);
    assert!((ground.anisotropy() - 2.0).abs() < 1e-6);
    assert!((ground.custom_angle_deg().unwrap() - 90.0).abs() < 0.2);
    assert!(ground.no_sweep());
    assert_eq!(FacetSurface::from_packed(ground.packed()), ground);
    // An angle is a line: 190 degrees is 10.
    let wrapped = FacetSurface::frosted().with_custom_angle(190.0);
    assert!((wrapped.custom_angle_deg().unwrap() - 10.0).abs() < 0.2);
    // Zero roughness is ideal.
    assert_eq!(FacetSurface::polished(0.0, 3.0), FacetSurface::IDEAL);
}

#[test]
fn the_halo_width_interpolates_the_presets() {
    for preset in [
        PolishPreset::Mirror,
        PolishPreset::Fine,
        PolishPreset::Commercial,
        PolishPreset::Prepolish,
    ] {
        assert!((halo_alpha(preset.rq_nm()) / preset.halo_alpha() - 1.0).abs() < 1e-3);
    }
    assert_eq!(halo_alpha(0.0), 0.0);
    let mut last = 0.0;
    for step in 1..200 {
        let alpha = halo_alpha(step as f32 * 0.5);
        assert!(alpha > last, "not monotonic at {step}");
        last = alpha;
    }
}

#[test]
fn an_arm_through_the_centre_scratches_parallel_to_the_girdle() {
    let machine = FacetingMachine::DEFAULT;
    assert_eq!(machine.sample_angle_deg(0.0), 0.0);
    assert_eq!(machine.sample_angle_deg(0.77), 0.0);
    assert_eq!(machine.nominal_angle_deg(), 0.0);
    let offset = FacetingMachine {
        arm_offset_deg: 12.0,
        ..machine
    };
    assert_eq!(offset.sample_angle_deg(0.3), 12.0);
    // The facet frame: the girdle direction lies in the horizontal plane.
    let normal = Vec3::new(0.6, 0.7, 0.2).normalize();
    let along = scratch_direction(normal, 0.0);
    assert!(along.y.abs() < 1e-6 && along.dot(normal).abs() < 1e-6);
    // 90 degrees about the normal is perpendicular to it, in the facet plane.
    let turned = scratch_direction(normal, 90.0);
    assert!(turned.dot(along).abs() < 1e-5 && turned.dot(normal).abs() < 1e-6);
    // A table facet takes model +X.
    assert!((scratch_direction(Vec3::Y, 0.0) - Vec3::X).length() < 1e-6);
}

#[test]
fn custom_angle_and_no_sweep_override_the_fan() {
    let machine = FacetingMachine {
        mast_distance_mm: 30.0,
        ..FacetingMachine::DEFAULT
    };
    let swept = FacetSurface::from_preset(PolishPreset::Fine, 0.3);
    assert_ne!(
        scratch_angle_deg(&swept, &machine, 0.1),
        scratch_angle_deg(&swept, &machine, 0.9)
    );
    let still = swept.with_no_sweep();
    assert_eq!(
        scratch_angle_deg(&still, &machine, 0.1),
        scratch_angle_deg(&still, &machine, 0.9)
    );
    assert_eq!(
        scratch_angle_deg(&still, &machine, 0.1),
        machine.nominal_angle_deg()
    );
    let fixed = swept.with_custom_angle(45.0);
    assert!((scratch_angle_deg(&fixed, &machine, 0.4) - 45.0).abs() < 0.2);
}

/// The sampled scratch angles against the analytic density of a uniform sweep in contact radius.
#[test]
#[expect(
    clippy::suboptimal_flops,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test arithmetic written as the formula; histogram bins are small non-negative indices"
)]
fn the_sweep_angle_histogram_matches_the_analytic_density() {
    let machine = FacetingMachine {
        arm_offset_deg: 5.0,
        lap_radius_mm: 100.0,
        mast_distance_mm: 30.0,
        sweep: SweepPreset::Typical.range(100.0),
    };
    let (lo, hi) = machine.angle_range_deg();
    assert!(lo < hi && lo > 5.0);
    let bins = 20usize;
    let draws = 100_000u32;
    let mut counts = vec![0u32; bins];
    for i in 0..draws {
        let angle = machine.sample_angle_deg((i as f32 + 0.5) / draws as f32);
        let bin = (((angle - lo) / (hi - lo)) * bins as f32) as usize;
        counts[bin.min(bins - 1)] += 1;
    }
    let width = (hi - lo) / bins as f32;
    let mut total = 0.0;
    for (bin, &count) in counts.iter().enumerate() {
        let sub = 200usize;
        let mut expected = 0.0f32;
        for s in 0..sub {
            let angle = lo + width * (bin as f32 + (s as f32 + 0.5) / sub as f32);
            expected += machine.angle_density_per_deg(angle) * width / sub as f32;
        }
        total += expected;
        let observed = count as f32 / draws as f32;
        assert!(
            (observed - expected).abs() < 2e-3,
            "bin {bin}: observed {observed}, expected {expected}"
        );
    }
    assert!((total - 1.0).abs() < 1e-2, "total {total}");
    assert_eq!(machine.angle_density_per_deg(lo - 1.0), 0.0);
    // Wider sweeps fan out more.
    let narrow = FacetingMachine {
        sweep: SweepPreset::Narrow.range(100.0),
        ..machine
    };
    let wide = FacetingMachine {
        sweep: SweepPreset::Wide.range(100.0),
        ..machine
    };
    let span = |m: &FacetingMachine| m.angle_range_deg().1 - m.angle_range_deg().0;
    assert!(span(&narrow) < span(&machine) && span(&machine) < span(&wide));
}
