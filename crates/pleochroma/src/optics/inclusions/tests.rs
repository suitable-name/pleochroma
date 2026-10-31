//! Tests of the inclusion types, the catalogue, the frame resolution and the two phase
//! functions (analytic normalisation, sampling against evaluation, cone conservation).

use super::{
    CrystalDir, CrystalFrame, Family, FamilyKind, InclusionSet, ResolvedFamily,
    catalogue::{PRESET_MATERIALS, catalogue_arm, find_preset, presets_for},
    needle::{needle_pdf, needle_sample},
    perpendicular_basis,
    platelet::{platelet_pdf, platelet_sample},
    spec::{DEFAULT_DENSITY, InclusionSpec, Sheen, SilkColor, density_factor},
    spectral::{snap_exponent, spectral_factor},
};
use crate::optics::{birefringence::alpha_axis_from_turn, materials::GemMaterial};
use glam::Vec3;
use std::f32::consts::PI;

fn builtin(name: &str) -> GemMaterial {
    GemMaterial::built_in()
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no built-in {name}"))
        .clone()
}

/// A small deterministic generator of `[0, 1)` randoms.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 16_777_216.0
    }
}

fn unit_vec(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z).normalize()
}

fn spec(preset: &str) -> InclusionSpec {
    InclusionSpec {
        preset: preset.to_string(),
        density: DEFAULT_DENSITY,
        turn_deg: None,
        sheen: None,
        color: None,
    }
}

fn same_line(a: Vec3, b: Vec3) -> bool {
    a.dot(b).abs() > 1.0 - 1e-5
}

#[test]
fn every_builtin_has_an_explicit_catalogue_arm() {
    for material in GemMaterial::built_in() {
        assert!(
            catalogue_arm(&material.name).is_some(),
            "{} has no catalogue arm",
            material.name
        );
    }
}

#[test]
fn preset_materials_cover_every_arm_with_presets() {
    for name in PRESET_MATERIALS {
        assert!(!presets_for(name).is_empty(), "{name} has no presets");
    }
    for material in GemMaterial::built_in() {
        for preset in presets_for(&material.name) {
            assert!(
                PRESET_MATERIALS
                    .iter()
                    .any(|listed| find_preset(listed, preset.name).is_some()),
                "{} of {} is not reachable through PRESET_MATERIALS",
                preset.name,
                material.name
            );
        }
    }
}

#[test]
fn every_preset_resolves_for_its_materials_and_is_in_range() {
    for material in GemMaterial::built_in() {
        for preset in presets_for(&material.name) {
            assert!(!preset.families.is_empty(), "{}", preset.name);
            assert!(preset.families.len() <= super::MAX_FAMILIES);
            assert!((-1.0..1.0).contains(&preset.haze_g));
            for family in preset.families {
                assert!(family.sigma > 0.0 && family.sigma < 1.0, "{}", preset.name);
                assert_eq!(snap_exponent(family.p), family.p, "{}", preset.name);
            }
            let resolved = spec(preset.name)
                .resolve(material, None)
                .unwrap_or_else(|| panic!("{} on {}", preset.name, material.name));
            assert_eq!(resolved.set.families().len(), preset.families.len());
        }
    }
}

#[test]
fn the_density_factor_is_one_at_the_default_density() {
    assert!((density_factor(DEFAULT_DENSITY) - 1.0).abs() < 0.03);
    assert!(density_factor(0.0).abs() < 1e-12);
    assert!(density_factor(f32::NAN).abs() < 1e-12);
    assert!((density_factor(5.0) - 8.0).abs() < 1e-6);
}

#[test]
fn the_spectral_factor_multiplies_only() {
    assert_eq!(spectral_factor(440.0, 0), 1.0);
    assert!((spectral_factor(440.0, 1) - 1.25).abs() < 1e-6);
    assert!((spectral_factor(440.0, 2) - 1.5625).abs() < 1e-6);
    assert!((spectral_factor(440.0, 4) - 1.25_f32.powi(4)).abs() < 1e-5);
    assert_eq!(spectral_factor(550.0, 4), 1.0);
    assert_eq!(snap_exponent(3), 2);
    assert_eq!(snap_exponent(9), 4);
}

/// Star Silk is three needles perpendicular to c, 60 degrees apart as lines.
#[test]
fn star_silk_is_three_needles_in_the_basal_plane_sixty_degrees_apart() {
    let mut sapphire = builtin("Sapphire");
    sapphire.c_axis = unit_vec(1.0, 0.0, 0.0);
    let resolved = spec("Star Silk")
        .resolve(&sapphire, None)
        .expect("resolves");
    let dirs: Vec<Vec3> = resolved.set.families().iter().map(|f| f.dir).collect();
    assert_eq!(dirs.len(), 3);
    for d in &dirs {
        assert!(d.dot(sapphire.c_axis).abs() < 1e-5, "perpendicular to c");
        assert!((d.length() - 1.0).abs() < 1e-5);
    }
    assert!(
        (dirs[0].dot(dirs[1]) - 0.5).abs() < 1e-5,
        "0 and 60 degrees"
    );
    assert!(
        (dirs[1].dot(dirs[2]) - 0.5).abs() < 1e-5,
        "60 and 120 degrees"
    );
    assert!(
        (dirs[0].dot(dirs[2]) + 0.5).abs() < 1e-5,
        "0 and 120 degrees"
    );
}

/// A 60 degree turn of the silk frame about c maps the three lines onto themselves: the
/// symmetry the six-rayed star rests on.
#[test]
fn a_sixty_degree_silk_turn_maps_the_star_onto_itself() {
    let mut ruby = builtin("Ruby");
    ruby.c_axis = Vec3::Y;
    let base = spec("Star Silk").resolve(&ruby, None).expect("base");
    let mut turned = spec("Star Silk");
    turned.turn_deg = Some(60.0);
    let turned = turned.resolve(&ruby, None).expect("turned");
    for family in turned.set.families() {
        assert!(
            base.set
                .families()
                .iter()
                .any(|b| same_line(b.dir, family.dir)),
            "{:?} is not one of the base lines",
            family.dir
        );
    }
}

/// The Silk Turn of a uniaxial stone is its stored Second Axis; the spec's own turn wins.
#[test]
fn the_silk_turn_of_a_uniaxial_stone_is_the_stored_second_axis() {
    let mut sapphire = builtin("Sapphire");
    sapphire.c_axis = Vec3::Y;
    assert!(sapphire.biaxial_delta_beta_alpha.is_none(), "uniaxial");
    let stored = alpha_axis_from_turn(Vec3::Y, 25.0);
    let resolved = spec("Fine Silk")
        .resolve(&sapphire, Some(stored))
        .expect("resolves");
    assert!(same_line(resolved.set.families()[0].dir, stored));
    let mut own = spec("Fine Silk");
    own.turn_deg = Some(70.0);
    let resolved = own.resolve(&sapphire, Some(stored)).expect("resolves");
    assert!(same_line(
        resolved.set.families()[0].dir,
        alpha_axis_from_turn(Vec3::Y, 70.0)
    ));
}

/// The silk follows the Crystal Axis: every needle stays perpendicular to the new axis.
#[test]
fn the_silk_follows_the_crystal_axis() {
    let mut sapphire = builtin("Sapphire");
    for axis in [
        unit_vec(0.3, 0.9, 0.1),
        unit_vec(-0.8, 0.2, 0.5),
        unit_vec(0.0, 0.0, 1.0),
    ] {
        sapphire.c_axis = axis;
        let resolved = spec("Fine Silk")
            .resolve(&sapphire, None)
            .expect("resolves");
        for family in resolved.set.families() {
            assert!(family.dir.dot(axis).abs() < 1e-5, "{axis:?}");
        }
    }
}

/// Chrysoberyl's needles run along `n_alpha` (the stone's second direction), not along c.
#[test]
fn cats_eye_silk_runs_along_the_alpha_axis_of_a_biaxial_stone() {
    let mut alexandrite = builtin("Alexandrite");
    assert!(alexandrite.biaxial_delta_beta_alpha.is_some(), "biaxial");
    alexandrite.c_axis = Vec3::Y;
    let alpha = alpha_axis_from_turn(Vec3::Y, 40.0);
    alexandrite.alpha_axis = Some(alpha);
    let resolved = spec("Cat's-Eye Silk")
        .resolve(&alexandrite, None)
        .expect("resolves");
    let families = resolved.set.families();
    assert_eq!(families.len(), 1);
    assert!(same_line(families[0].dir, alpha));
    assert!(families[0].dir.dot(alexandrite.c_axis).abs() < 1e-5);
}

#[test]
fn an_unknown_preset_or_zero_density_resolves_to_nothing_and_changes_nothing() {
    let sapphire = builtin("Sapphire");
    assert!(spec("Growth Tubes").resolve(&sapphire, None).is_none());
    assert_eq!(
        sapphire
            .clone()
            .with_inclusion_spec(&spec("Growth Tubes"), None),
        sapphire
    );
    let mut none = spec("Fine Silk");
    none.density = 0.0;
    assert!(none.resolve(&sapphire, None).is_none());
    assert_eq!(sapphire.clone().with_inclusion_spec(&none, None), sapphire);
    assert!(
        find_preset("Sapphire", "fine silk").is_some(),
        "case-insensitive"
    );
}

#[test]
fn a_material_scatters_only_with_haze_or_an_active_set() {
    for material in GemMaterial::built_in() {
        assert!(material.inclusions.is_none());
        assert_eq!(material.scatters(), material.scattering_sigma_s > 0.0);
    }
    let sapphire = builtin("Sapphire");
    assert!(!sapphire.scatters());
    let with_silk = sapphire
        .clone()
        .with_inclusion_spec(&spec("Fine Silk"), None);
    assert!(with_silk.scatters());
    assert!((with_silk.scattering_sigma_s - sapphire.scattering_sigma_s).abs() < 1e-12);
    // A set with no active family clears the field again.
    assert!(
        with_silk
            .with_inclusions(InclusionSet::EMPTY)
            .inclusions
            .is_none()
    );
}

#[test]
fn a_zoned_material_scatters_off_its_inclusions() {
    let ametrine = builtin("Ametrine");
    assert!(ametrine.zoning.is_some());
    let mut with_set = ametrine.clone();
    with_set.inclusions = Some(InclusionSet::new(&[ResolvedFamily {
        kind: FamilyKind::Needle,
        dir: Vec3::Y,
        sigma: 0.2,
        p: 0,
        lobe: 0.1,
        rho: 0.0,
    }]));
    assert!(with_set.oriented_inclusions().is_some());
    assert!(with_set.scatters());
    // Without the set the zoned stone does not scatter.
    assert!(!ametrine.scatters());
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn sheen_and_color_change_the_lobe_and_the_exponent() {
    let mut sapphire = builtin("Sapphire");
    sapphire.c_axis = Vec3::Y;
    let base = spec("Fine Silk").resolve(&sapphire, None).expect("base");
    let mut soft = spec("Fine Silk");
    soft.sheen = Some(Sheen::Soft);
    soft.color = Some(SilkColor::Grey);
    let soft = soft.resolve(&sapphire, None).expect("soft");
    let (b, s) = (base.set.families()[0], soft.set.families()[0]);
    assert!((s.lobe - 2.0 * b.lobe).abs() < 1e-6);
    assert_eq!((b.p, s.p), (2, 0));
}

/// The Silk Turn turns about the CANONICAL sign of the Crystal Axis (upper hemisphere; on the
/// equator azimuth in `[0, 180)`), like the stored Second Axis, so "Silk Turn 30" and "Second Axis
/// 30" are the same line even for a slider vector that points the other way.
#[test]
fn the_silk_turn_is_about_the_canonical_axis_sign() {
    // Tilt 90, azimuth 200: the equator, with the slider vector on the z < 0 side. Tilt 120: the
    // lower hemisphere.
    let equator = Vec3::new(200f32.to_radians().cos(), 0.0, 200f32.to_radians().sin());
    let lower = unit_vec(0.4, -0.5, 0.3);
    for (c, canonical) in [(equator, -equator), (lower, -lower)] {
        let mut sapphire = builtin("Sapphire");
        sapphire.c_axis = c;
        let mut silk = spec("Fine Silk");
        silk.turn_deg = Some(30.0);
        // Fine Silk's first family is the basal turn 0: the reference a axis itself.
        let resolved = silk.resolve(&sapphire, None).expect("silk");
        let expected = alpha_axis_from_turn(canonical, 30.0);
        let got = resolved.set.families()[0].dir;
        assert!(
            (got - expected).length() < 1e-4 || (got + expected).length() < 1e-4,
            "{c:?}: {got:?} vs {expected:?}"
        );
        // The same line for either end of the axis.
        sapphire.c_axis = -c;
        let flipped = silk.resolve(&sapphire, None).expect("silk");
        let other = flipped.set.families()[0].dir;
        assert!(
            (got - other).length() < 1e-4 || (got + other).length() < 1e-4,
            "{c:?}: {got:?} vs {other:?}"
        );
    }
}

#[test]
fn the_set_is_capped_and_sanitised() {
    let good = ResolvedFamily {
        kind: FamilyKind::Needle,
        dir: Vec3::new(0.0, 2.0, 0.0),
        sigma: 0.1,
        p: 3,
        lobe: 7.0,
        rho: 5.0,
    };
    let bad = ResolvedFamily { sigma: 0.0, ..good };
    let set = InclusionSet::new(&[bad, good, good, good, good, good]);
    assert_eq!(set.families().len(), super::MAX_FAMILIES);
    let f = set.families()[0];
    assert!((f.dir.length() - 1.0).abs() < 1e-6, "direction normalised");
    assert_eq!(f.p, 2, "p snapped");
    assert!(f.lobe <= 1.0 && f.rho < 1.0);
    assert!(!InclusionSet::new(&[bad]).is_active());
}

/// An ideal cone (lobe 0, or a NaN lobe) is floored, and no built-in preset is touched by it,
/// Sharp sheen included.
#[test]
fn the_needle_lobe_is_floored_and_no_builtin_preset_changes() {
    let needle = ResolvedFamily {
        kind: FamilyKind::Needle,
        dir: Vec3::Y,
        sigma: 0.1,
        p: 0,
        lobe: 0.0,
        rho: 0.0,
    };
    for lobe in [0.0, -1.0, f32::NAN] {
        let set = InclusionSet::new(&[ResolvedFamily { lobe, ..needle }]);
        assert!((set.families()[0].lobe - super::NEEDLE_LOBE_FLOOR).abs() < 1e-9);
    }
    for material in super::catalogue::PRESET_MATERIALS {
        for preset in super::catalogue::presets_for(material) {
            for family in preset.families {
                assert!(
                    family.lobe * 0.5 >= super::NEEDLE_LOBE_FLOOR,
                    "{material} / {} is below the floor with Sharp sheen",
                    preset.name
                );
            }
        }
    }
}

#[test]
fn the_frame_is_right_handed_and_orthonormal() {
    let frame = CrystalFrame::new(unit_vec(0.2, 0.7, -0.4), Some(unit_vec(1.0, 1.0, 1.0)));
    assert!(frame.c.dot(frame.a1).abs() < 1e-5);
    assert!(frame.c.dot(frame.a2).abs() < 1e-5);
    assert!(frame.a1.dot(frame.a2).abs() < 1e-5);
    assert!((frame.a2 - frame.c.cross(frame.a1)).length() < 1e-6);
    let tilted = frame.direction(CrystalDir::Tilted {
        tilt_deg: 90.0,
        turn_deg: 90.0,
    });
    assert!((tilted - frame.a2).length() < 1e-5);
    // A frame with no second direction uses the same completion as the biaxial optics.
    let bare = CrystalFrame::new(Vec3::Y, None);
    assert_eq!(bare.a1, alpha_axis_from_turn(Vec3::Y, 0.0));
}

/// The sphere integral of `pdf` around `axis`, on a grid in `(s, phi)` with the substitution
/// `s = 1 - 2 r^2` that removes a `1/r` singularity at the pole `s = 1`.
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn sphere_integral(axis: Vec3, pdf: impl Fn(Vec3) -> f32, n_r: usize, n_phi: usize) -> f64 {
    let (e1, e2) = perpendicular_basis(axis);
    let mut total = 0.0f64;
    for i in 0..n_r {
        let r = (i as f32 + 0.5) / n_r as f32;
        let s = 1.0 - 2.0 * r * r;
        let radial = s.mul_add(-s, 1.0).max(0.0).sqrt();
        let weight = 4.0 * r / n_r as f32 * (2.0 * PI / n_phi as f32);
        for j in 0..n_phi {
            let (sin, cos) = ((j as f32 + 0.5) * 2.0 * PI / n_phi as f32).sin_cos();
            let w = axis * s + (e1 * cos + e2 * sin) * radial;
            total += f64::from(pdf(w) * weight);
        }
    }
    total
}

/// The needle density integrates to one over the sphere for a spread of lobe widths,
/// forwardness and incoming directions.
#[test]
fn the_needle_pdf_is_normalised() {
    let t = unit_vec(0.2, 0.9, -0.3);
    for (beta, rho, w_i) in [
        (0.03, 0.5, unit_vec(1.0, 0.1, 0.4)),
        (0.15, 0.0, unit_vec(-0.4, 0.3, 0.8)),
        (0.2, 0.8, unit_vec(0.3, -0.8, 0.2)),
        (0.06, 0.3, unit_vec(0.9, 0.9, -0.1)),
    ] {
        let integral = sphere_integral_around_t(t, |w| needle_pdf(w_i, t, beta, rho, w));
        assert!(
            (integral - 1.0).abs() < 1e-2,
            "beta {beta} rho {rho}: integral {integral}"
        );
    }
}

/// The sphere integral in `(s, phi)` about the needle axis itself (the lobe is narrow in `s`).
fn sphere_integral_around_t(t: Vec3, pdf: impl Fn(Vec3) -> f32) -> f64 {
    let (e1, e2) = perpendicular_basis(t);
    let (n_s, n_phi) = (4000usize, 720usize);
    let mut total = 0.0f64;
    for i in 0..n_s {
        let s = -1.0 + 2.0 * (i as f32 + 0.5) / n_s as f32;
        let radial = s.mul_add(-s, 1.0).max(0.0).sqrt();
        for j in 0..n_phi {
            let (sin, cos) = ((j as f32 + 0.5) * 2.0 * PI / n_phi as f32).sin_cos();
            let w = t * s + (e1 * cos + e2 * sin) * radial;
            total += f64::from(pdf(w));
        }
    }
    total * f64::from(2.0 / n_s as f32) * f64::from(2.0 * PI / n_phi as f32)
}

/// With an ideal cone (`beta == 0`) every sample keeps the component along the axis.
#[test]
fn an_ideal_needle_cone_conserves_the_axial_component() {
    let mut rng = Rng(0x1234_5678);
    for _ in 0..2000 {
        let t = unit_vec(rng.next() - 0.5, rng.next() - 0.5, rng.next() - 0.5);
        let w_i = unit_vec(rng.next() - 0.5, rng.next() - 0.5, rng.next() - 0.5);
        let w_o = needle_sample(w_i, t, 0.0, 0.5, rng.next(), rng.next());
        assert!((w_o.dot(t) - w_i.dot(t)).abs() < 1e-5);
        assert!((w_o.length() - 1.0).abs() < 1e-5);
    }
}

/// A histogram of sampled `s` and `phi` against the analytic densities.
#[test]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::manual_midpoint,
    reason = "histogram bins are small non-negative indices; the bin formula is written out"
)]
fn needle_samples_follow_the_evaluated_density() {
    let t = unit_vec(0.1, 1.0, 0.2);
    let w_i = unit_vec(0.8, 0.3, -0.4);
    let (beta, rho) = (0.12, 0.6);
    let (e_u, _) = {
        let perp = (w_i - t * w_i.dot(t)).normalize();
        (perp, t.cross(perp))
    };
    let e_v = t.cross(e_u);
    let n = 200_000;
    let (bins_s, bins_phi) = (40usize, 36usize);
    let mut hist_s = vec![0u32; bins_s];
    let mut hist_phi = vec![0u32; bins_phi];
    let mut rng = Rng(0x9E37_79B9);
    for _ in 0..n {
        let w_o = needle_sample(w_i, t, beta, rho, rng.next(), rng.next());
        let s = w_o.dot(t);
        let bin = (((s + 1.0) * 0.5 * bins_s as f32) as usize).min(bins_s - 1);
        hist_s[bin] += 1;
        let perp = w_o - t * s;
        let phi = perp.dot(e_v).atan2(perp.dot(e_u));
        let bin = (((phi + PI) / (2.0 * PI) * bins_phi as f32) as usize).min(bins_phi - 1);
        hist_phi[bin] += 1;
    }
    // Expected mass per bin by fine numerical integration of the evaluated densities.
    let xi = w_i.dot(t);
    let chi2_s = chi_square(&hist_s, n, |k| {
        integrate(
            -1.0 + 2.0 * k as f32 / bins_s as f32,
            -1.0 + 2.0 * (k + 1) as f32 / bins_s as f32,
            |s| super::needle::needle_s_density(xi, s, beta),
        )
    });
    let chi2_phi = chi_square(&hist_phi, n, |k| {
        integrate(
            -PI + 2.0 * PI * k as f32 / bins_phi as f32,
            -PI + 2.0 * PI * (k + 1) as f32 / bins_phi as f32,
            |phi| super::needle::wrapped_cauchy_density(phi.cos(), rho),
        )
    });
    assert!(chi2_s < 3.0, "s histogram chi2 per bin {chi2_s}");
    assert!(chi2_phi < 3.0, "phi histogram chi2 per bin {chi2_phi}");
}

#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn integrate(a: f32, b: f32, f: impl Fn(f32) -> f32) -> f64 {
    let steps = 400;
    let h = (b - a) / steps as f32;
    (0..steps)
        .map(|i| f64::from(f(a + (i as f32 + 0.5) * h)) * f64::from(h))
        .sum()
}

/// The reduced chi-square of a histogram against expected bin masses.
fn chi_square(hist: &[u32], n: usize, expected_mass: impl Fn(usize) -> f64) -> f64 {
    let mut total = 0.0;
    let mut bins = 0.0;
    for (k, observed) in hist.iter().enumerate() {
        let expected = expected_mass(k) * n as f64;
        if expected < 5.0 {
            continue;
        }
        total += (f64::from(*observed) - expected).powi(2) / expected;
        bins += 1.0;
    }
    total / bins
}

/// Reciprocity `sigma(w_i) p(w_i -> w_o) = sigma(-w_o) p(-w_o -> -w_i)` for the needle holds in
/// the limit of a thin cone. The finite logistic blur breaks it by a relative amount of order
/// the blur times the lobe's sine slope, so it is checked at a small scale.
#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn the_needle_phase_is_nearly_reciprocal_for_a_thin_cone() {
    let t = unit_vec(0.0, 1.0, 0.0);
    let (beta, rho) = (0.004, 0.4);
    let w_i = unit_vec(0.8, 0.2, 0.3);
    let xi = w_i.dot(t);
    for k in -2..=2 {
        let s = xi + k as f32 * beta;
        let radial = (1.0 - s * s).sqrt();
        let (e1, e2) = perpendicular_basis(t);
        let w_o = t * s + (e1 * 0.6 + e2 * 0.8) * radial;
        let sigma = |w: Vec3| (1.0 - w.dot(t).powi(2)).sqrt();
        let forward = sigma(w_i) * needle_pdf(w_i, t, beta, rho, w_o);
        let backward = sigma(-w_o) * needle_pdf(-w_o, t, beta, rho, -w_i);
        assert!(
            (forward / backward - 1.0).abs() < 0.05,
            "k {k}: {forward} vs {backward}"
        );
    }
}

/// The platelet density integrates to one over the sphere.
#[test]
fn the_platelet_pdf_is_normalised() {
    for (alpha, m, w_i) in [
        (0.25, unit_vec(0.0, 1.0, 0.0), unit_vec(0.6, 0.7, 0.2)),
        (0.1, unit_vec(0.3, 0.8, 0.5), unit_vec(-0.5, 0.6, 0.6)),
        (0.3, unit_vec(1.0, 0.2, 0.0), unit_vec(0.2, -0.7, 0.6)),
    ] {
        let integral = sphere_integral(w_i, |w| platelet_pdf(w_i, m, alpha, w), 2000, 720);
        assert!(
            (integral - 1.0).abs() < 2e-2,
            "alpha {alpha}: integral {integral}"
        );
    }
}

/// The flake tilt of platelet samples follows GGX: `tan^2 / (alpha^2 + tan^2)` is uniform.
#[test]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "histogram bins are small non-negative indices"
)]
fn platelet_samples_follow_the_ggx_distribution() {
    let m = unit_vec(0.2, 1.0, 0.1);
    let w_i = unit_vec(0.7, 0.4, -0.3);
    let alpha = 0.25;
    let n = 100_000;
    let bins = 40usize;
    let mut hist = vec![0u32; bins];
    let mut rng = Rng(0xC0FF_EE11);
    for _ in 0..n {
        let w_o = platelet_sample(w_i, m, alpha, rng.next(), rng.next());
        let h = (w_o - w_i).normalize();
        let cos_m = h.dot(m).abs();
        let tan2 = cos_m.mul_add(-cos_m, 1.0) / (cos_m * cos_m);
        let u = tan2 / alpha.mul_add(alpha, tan2);
        hist[((u * bins as f32) as usize).min(bins - 1)] += 1;
    }
    let chi2 = chi_square(&hist, n, |_| 1.0 / bins as f64);
    assert!(chi2 < 3.0, "chi2 per bin {chi2}");
}

/// A near-ideal platelet mirrors the ray about the mean normal.
#[test]
fn a_thin_platelet_family_reflects_like_a_mirror() {
    let m = unit_vec(0.0, 1.0, 0.0);
    let w_i = unit_vec(0.6, -0.5, 0.2);
    let mirror = w_i - m * (2.0 * w_i.dot(m));
    // GGX has long tails: a few flakes are steep even at alpha 0.02 (about 14 % tilt past
    // 0.05 rad), so the check is on the bulk, not on every sample.
    let mut rng = Rng(7);
    let near = (0..200)
        .filter(|_| {
            let w_o = platelet_sample(w_i, m, 0.02, rng.next(), rng.next());
            (w_o - mirror).length() < 0.2
        })
        .count();
    assert!(
        near >= 160,
        "{near} of 200 samples near the mirror direction {mirror:?}"
    );
}

/// `Family` values survive the catalogue: all four exponents appear only as allowed ones.
#[test]
fn the_catalogue_uses_only_allowed_exponents() {
    for material in GemMaterial::built_in() {
        for preset in presets_for(&material.name) {
            for family in preset.families {
                let f: &Family = family;
                assert!([0, 1, 2, 4].contains(&f.p));
            }
        }
    }
}

/// The render path: without a spec the result is exactly `apply_material_overrides`; with one
/// the families sit in the frame of the overridden Crystal Axis and the preset's haze `g`
/// replaces the default.
#[test]
fn the_render_override_resolves_the_spec_in_the_oriented_frame() {
    use crate::render_setup::{
        MaterialOverrides, apply_material_overrides, apply_material_overrides_with_inclusions,
    };
    let overrides = MaterialOverrides {
        inclusion_sigma_s: 0.2,
        c_axis_override: Some(unit_vec(0.0, 0.0, 1.0)),
        alpha_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: 0.0,
    };
    let sapphire = builtin("Sapphire");
    let plain = apply_material_overrides(sapphire.clone(), &overrides, None);
    assert_eq!(
        apply_material_overrides_with_inclusions(sapphire.clone(), &overrides, None, None),
        plain
    );
    let silky = apply_material_overrides_with_inclusions(
        sapphire,
        &overrides,
        None,
        Some(&spec("Fine Silk")),
    );
    let set = silky.inclusions.expect("silk on");
    assert_eq!(set.families().len(), 3);
    for family in set.families() {
        assert!(
            family.dir.dot(Vec3::Z).abs() < 1e-5,
            "perpendicular to the new c"
        );
    }
    assert!((silky.scattering_g - 0.5).abs() < 1e-6, "species haze g");
    assert!(plain.inclusions.is_none());
    assert!((plain.scattering_g - GemMaterial::DEFAULT_SCATTERING_G).abs() < 1e-6);
}
