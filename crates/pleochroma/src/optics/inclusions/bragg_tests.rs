//! Tests of the Bragg structure: the Bragg wavelength against its formula, the spectral profile,
//! the majorant, the deterministic domain map, the per-design seed and the new built-ins.

use super::{
    InclusionSet,
    bragg::{
        BraggStructure, DEFAULT_SEED, LABRADORITE_NORMAL, LAMELLA_INDEX, StructureKind,
        StructurePattern, StructureSpec, bragg_reflect, bragg_wavelength_nm, lamella_wavelength_nm,
        opal_bragg_ratio, opal_diameter_nm, opal_effective_index, opal_wavelength_nm,
        spectral_profile, structure_kind,
    },
    catalogue::catalogue_arm,
    domains::{cell_of, domain_at, domain_of_cell, seed_point},
    spec::{DEFAULT_DENSITY, InclusionSpec},
};
use crate::optics::materials::GemMaterial;
use glam::Vec3;

fn builtin(name: &str) -> GemMaterial {
    GemMaterial::built_in()
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no built-in {name}"))
        .clone()
}

fn opal_structure(seed: u32) -> BraggStructure {
    BraggStructure::new(seed, 0.2, (440.0, 680.0), 0.05, 0.8, 0.6, 0.03, Vec3::ZERO)
}

fn relative_error(value: f32, expected: f64) -> f64 {
    ((f64::from(value) - expected) / expected).abs()
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn opal_bragg_wavelength_follows_the_lattice_formula() {
    // n_eff^2 = 0.74 * 1.45^2 + 0.26 * 1.365^2, lambda_B(0) = 2 sqrt(2/3) D n_eff.
    let n_eff = (0.74f64 * 1.45 * 1.45 + 0.26 * 1.365 * 1.365).sqrt();
    assert!(relative_error(opal_effective_index(), n_eff) < 1e-6);
    let ratio = 2.0 * (2.0f64 / 3.0).sqrt() * n_eff;
    assert!(relative_error(opal_bragg_ratio(), ratio) < 1e-6);
    // The plan's "about 2.33 D": D = 200 nm is blue, D = 270 nm red.
    assert!((opal_bragg_ratio() - 2.33).abs() < 0.01);
    assert!((opal_wavelength_nm(200.0) - 466.0).abs() < 2.0);
    assert!((opal_wavelength_nm(270.0) - 630.0).abs() < 2.0);
    assert!(relative_error(opal_diameter_nm(opal_wavelength_nm(240.0)), 240.0) < 1e-6);
}

#[test]
fn labradorite_bragg_wavelength_follows_two_n_d() {
    assert!(relative_error(lamella_wavelength_nm(193.0), 2.0 * 1.56 * 193.0) < 1e-6);
    assert!((LAMELLA_INDEX - 1.56).abs() < 1e-6);
    // The published pair period of 193 nm gives the yellow of the paper (597 nm) to within 1 %.
    assert!((lamella_wavelength_nm(193.0) - 597.0).abs() / 597.0 < 0.01);
    // 150 nm is blue, 250 nm is at the red edge.
    assert!(lamella_wavelength_nm(150.0) < 480.0);
    assert!(lamella_wavelength_nm(250.0) > 760.0);
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the cosine of a test angle fits an f32"
)]
fn the_bragg_wavelength_shortens_with_the_cosine_of_the_angle() {
    let lambda_b0 = 600.0f32;
    for angle_deg in [0.0f64, 15.0, 30.0, 60.0] {
        let expected = 600.0 * angle_deg.to_radians().cos();
        let got = bragg_wavelength_nm(lambda_b0, angle_deg.to_radians().cos() as f32);
        assert!(relative_error(got, expected) < 1e-6, "{angle_deg}: {got}");
    }
    // The sign of the normal does not matter.
    assert_eq!(
        bragg_wavelength_nm(600.0, -0.5),
        bragg_wavelength_nm(600.0, 0.5)
    );
}

#[test]
fn the_spectral_profile_peaks_at_the_bragg_wavelength_with_the_stated_width() {
    let purity = 0.05;
    assert!((spectral_profile(550.0, 550.0, purity) - 1.0).abs() < 1e-6);
    let one_sigma = 550.0 * (1.0 + purity);
    assert!((spectral_profile(one_sigma, 550.0, purity) - (-0.5f32).exp()).abs() < 1e-5);
    assert!(spectral_profile(700.0, 550.0, purity) < 1e-6);
    // The second order shows while lambda_B / 2 >= 380 nm: a peak at 400 nm of 0.3.
    assert!((spectral_profile(400.0, 800.0, purity) - 0.3).abs() < 1e-4);
    // ... and is gone below it.
    assert!(spectral_profile(350.0, 700.0, purity) < 1e-6);
    assert_eq!(spectral_profile(500.0, 0.0, purity), 0.0);
}

#[test]
fn the_majorant_bounds_the_coefficient_everywhere() {
    for structure in [
        opal_structure(3),
        BraggStructure::new(4, 0.1, (500.0, 820.0), 0.3, 1.0, 0.9, 0.0, Vec3::Z),
    ] {
        let bound = structure.majorant();
        assert!(bound >= structure.kappa());
        for i in 0..400u32 {
            let p = Vec3::new(i as f32 * 0.037, i as f32 * -0.021, i as f32 * 0.013);
            let domain = domain_at(&structure, p);
            for cos in [0.0f32, 0.2, 0.5, 0.8, 1.0] {
                for step in 0..=80u8 {
                    let lambda = f32::from(step).mul_add(5.0, 380.0);
                    let sigma = structure.sigma(&domain, cos, lambda);
                    assert!(sigma <= bound * (1.0 + 1e-6), "{sigma} > {bound}");
                }
            }
        }
    }
}

#[test]
fn kappa_is_the_log_reflectance_over_the_domain_size() {
    let s = BraggStructure::new(1, 0.2, (500.0, 600.0), 0.05, 1.0, 0.5, 0.0, Vec3::ZERO);
    assert!((s.kappa() - (-(0.5f32).ln() / 0.2)).abs() < 1e-5);
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn a_reflection_without_mosaic_is_the_exact_mirror() {
    let g = Vec3::new(0.2, 0.9, -0.3).normalize();
    for (i, w) in [Vec3::X, Vec3::new(0.4, -0.8, 0.2).normalize(), -Vec3::Z]
        .into_iter()
        .enumerate()
    {
        let expected = w - 2.0 * w.dot(g) * g;
        let got = bragg_reflect(w, g, 0.0, 0.1 + 0.2 * i as f32, 0.7);
        assert!((got - expected).length() < 1e-6, "{got} vs {expected}");
    }
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn the_domain_map_is_a_pure_function_of_the_seed_and_the_point() {
    let a = opal_structure(1234);
    let b = opal_structure(1234);
    let other = opal_structure(1235);
    let mut differing = 0;
    for i in 0..10_000u32 {
        let p = Vec3::new(
            (i % 37) as f32 * 0.113,
            (i % 91) as f32 * 0.071 - 3.0,
            (i / 100) as f32 * 0.059,
        );
        let (da, db) = (domain_at(&a, p), domain_at(&b, p));
        assert_eq!(da, db);
        if domain_at(&other, p).id != da.id {
            differing += 1;
        }
    }
    assert!(differing > 9_900, "a new seed is a new map: {differing}");
}

#[test]
fn a_seed_point_belongs_to_its_own_domain() {
    let s = opal_structure(77);
    for i in 0..500i32 {
        let cell = [i % 7 - 3, i % 11 - 5, i % 13 - 6];
        let p = seed_point(&s, cell);
        assert_eq!(
            cell_of(s.cell_size, p),
            cell,
            "the jitter stays in the cell"
        );
        assert_eq!(domain_at(&s, p).cell, cell);
        assert_eq!(domain_at(&s, p), domain_of_cell(&s, cell));
    }
}

#[test]
fn domain_properties_follow_the_structure() {
    let s = opal_structure(9);
    let mut active = 0u32;
    let mut normals_differ = false;
    let first = domain_of_cell(&s, [0, 0, 0]).normal;
    let n = 4000u32;
    for i in 0..n {
        let j = i.cast_signed();
        let d = domain_of_cell(&s, [j % 20, (j / 20) % 20, j / 400]);
        assert!(d.lambda_b0_nm >= 440.0 && d.lambda_b0_nm <= 680.0);
        assert!((d.normal.length() - 1.0).abs() < 1e-5);
        normals_differ |= (d.normal - first).length() > 1e-3;
        active += u32::from(d.active);
    }
    let fraction = f64::from(active) / f64::from(n);
    assert!((fraction - 0.8).abs() < 0.03, "coverage {fraction}");
    assert!(normals_differ, "an opal's lattice normals are random");

    // Labradorite: one normal for every domain.
    let normal = Vec3::new(0.0, 0.6, 0.8);
    let lab = BraggStructure::new(9, 0.5, (470.0, 740.0), 0.06, 0.9, 0.5, 0.1, normal);
    for i in 0..200i32 {
        let d = domain_of_cell(&lab, [i, -i, i / 3]);
        assert!((d.normal - normal).length() < 1e-6);
    }
}

#[test]
fn the_structure_values_are_limited_to_their_ranges() {
    let s = BraggStructure::new(
        1,
        f32::NAN,
        (900.0, 100.0),
        5.0,
        2.0,
        1.5,
        9.0,
        Vec3::new(0.0, 3.0, 4.0),
    );
    assert!(s.cell_size > 0.0 && s.cell_size.is_finite());
    assert!(s.lambda_low_nm <= s.lambda_high_nm);
    assert_eq!(s.purity, 0.3);
    assert_eq!(s.coverage, 1.0);
    assert_eq!(s.peak_reflectance, 0.95);
    assert_eq!(s.mosaic, 0.5);
    assert!((s.normal - Vec3::new(0.0, 0.6, 0.8)).length() < 1e-6);
}

#[test]
fn only_the_structural_color_stones_have_a_structure() {
    for name in ["Precious Opal (White)", "Precious Opal (Black)"] {
        assert_eq!(structure_kind(name), Some(StructureKind::Opal));
        assert!(StructureSpec::default_for(name).is_some());
    }
    assert_eq!(
        structure_kind("Labradorite"),
        Some(StructureKind::Labradorite)
    );
    for name in ["Opal", "Moonstone", "Quartz", "Diamond"] {
        assert_eq!(structure_kind(name), None, "{name}");
        assert!(StructureSpec::default_for(name).is_none());
        assert!(
            StructureSpec {
                pattern: StructurePattern::Harlequin,
                seed: 1,
                size: None,
                color_low_nm: 450.0,
                color_high_nm: 650.0,
                purity: 0.05,
                coverage: 0.8,
            }
            .resolve(&builtin(name))
            .is_none()
        );
    }
}

#[test]
fn the_seed_is_per_design_and_changes_the_map_only() {
    let material = builtin("Precious Opal (White)");
    let mut spec = StructureSpec::default_for(&material.name).expect("opal has a default");
    assert_eq!(spec.seed, DEFAULT_SEED);
    let a = spec.resolve(&material).expect("opal resolves");
    spec.seed = 99;
    let b = spec.resolve(&material).expect("opal resolves");
    assert_ne!(a.seed, b.seed);
    assert_eq!(
        BraggStructure { seed: 0, ..a },
        BraggStructure { seed: 0, ..b },
        "the seed is the only difference"
    );
    let p = Vec3::new(0.3, 0.1, -0.2);
    assert_ne!(domain_at(&a, p).id, domain_at(&b, p).id);
}

#[test]
fn the_pattern_sets_the_cell_size_unless_the_design_overrides_it() {
    let material = builtin("Precious Opal (White)");
    let mut spec = StructureSpec::default_for(&material.name).expect("opal has a default");
    for pattern in [
        StructurePattern::Pinfire,
        StructurePattern::Harlequin,
        StructurePattern::BroadFlash,
    ] {
        spec.pattern = pattern;
        let got = spec.resolve(&material).expect("resolves");
        assert!((got.cell_size - pattern.cell_size()).abs() < 1e-6);
    }
    spec.size = Some(0.33);
    assert!((spec.resolve(&material).expect("resolves").cell_size - 0.33).abs() < 1e-6);
    assert!(StructurePattern::Pinfire.cell_size() < StructurePattern::Harlequin.cell_size());
    assert!(StructurePattern::Harlequin.cell_size() < StructurePattern::BroadFlash.cell_size());
}

/// `resolve_as` is `resolve` for a built-in of that kind, and gives a custom material (any name)
/// the structure of the kind it imitates.
#[test]
fn resolve_as_reads_a_custom_material_as_the_kind_it_names() {
    for (name, kind) in [
        ("Precious Opal (White)", StructureKind::Opal),
        ("Labradorite", StructureKind::Labradorite),
    ] {
        let material = builtin(name);
        let spec = StructureSpec::default_for(name).expect("has a default");
        assert_eq!(
            spec.resolve(&material),
            Some(spec.resolve_as(kind, &material))
        );
        let mut custom = material.clone();
        custom.name = "My Stone".into();
        assert!(spec.resolve(&custom).is_none());
        assert_eq!(
            spec.resolve_as(kind, &custom),
            spec.resolve_as(kind, &material)
        );
        let applied = custom.with_structure_spec_as(kind, &spec);
        assert!(
            applied
                .inclusions
                .is_some_and(|set| set.structure.is_some())
        );
        assert_eq!(StructureKind::from_name(kind.name()), Some(kind));
    }
    assert_eq!(StructureKind::from_name("quartz"), None);
}

/// The lamella normal is fixed in the crystal frame, so it turns with the Crystal Axis: the angle
/// to the stone's `n_gamma` axis is the same whichever way the axis points.
#[test]
fn the_labradorite_flash_follows_the_crystal_axis() {
    let spec = StructureSpec::default_for("Labradorite").expect("labradorite has a default");
    let super::CrystalDir::Tilted { tilt_deg, .. } = LABRADORITE_NORMAL else {
        panic!("the lamella normal is a tilted direction");
    };
    let mut normals = Vec::new();
    for axis in [Vec3::Y, Vec3::X, Vec3::new(1.0, 1.0, 0.3).normalize()] {
        let mut material = builtin("Labradorite");
        material.c_axis = axis;
        let structure = spec.resolve(&material).expect("labradorite resolves");
        assert!((structure.normal.length() - 1.0).abs() < 1e-5);
        let angle = structure
            .normal
            .dot(axis)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        assert!((angle - tilt_deg).abs() < 0.01, "{angle}");
        normals.push(structure.normal);
    }
    assert!(
        (normals[0] - normals[1]).length() > 0.1,
        "the flash turns with the axis"
    );
}

#[test]
fn with_structure_adds_to_the_families_and_makes_the_stone_scatter() {
    let plain = builtin("Precious Opal (White)");
    assert!(plain.inclusions.is_none() && !plain.scatters());
    let structure = opal_structure(5);
    let stone = plain.with_structure(structure);
    assert!(stone.scatters());
    assert_eq!(
        stone
            .oriented_inclusions()
            .and_then(InclusionSet::structure),
        Some(&structure)
    );

    // A moonstone keeps its platelet family when a structure is added.
    let moonstone = builtin("Moonstone")
        .with_inclusion_spec(
            &InclusionSpec {
                preset: "Adularescence".to_string(),
                density: DEFAULT_DENSITY,
                turn_deg: None,
                sheen: None,
                color: None,
            },
            None,
        )
        .with_structure(structure);
    let set = moonstone.oriented_inclusions().expect("active");
    assert_eq!(set.families().len(), 1);
    assert!(set.structure().is_some());

    // A spec for a material without a structure changes nothing.
    let spec = StructureSpec::default_for("Precious Opal (White)").expect("default");
    let quartz = builtin("Quartz");
    assert_eq!(quartz.clone().with_structure_spec(&spec), quartz);
    // ... and an empty set clears everything.
    assert!(
        stone
            .with_inclusions(InclusionSet::EMPTY)
            .inclusions
            .is_none()
    );
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test arithmetic written as the formula"
)]
fn the_new_built_ins_have_their_catalogue_and_optics() {
    let names = [
        "Precious Opal (White)",
        "Precious Opal (Black)",
        "Labradorite",
        "Moonstone",
    ];
    for name in names {
        let material = builtin(name);
        assert!(catalogue_arm(name).is_some(), "{name}");
        assert!(material.inclusions.is_none() && material.zoning.is_none());
        assert_eq!(material.scattering_sigma_s, 0.0);
        assert!(material.recommended_scattering().0 > 0.0, "{name}");
    }
    let opal = builtin("Opal");
    for name in &names[..2] {
        let precious = builtin(name);
        assert_eq!(precious.dispersion, opal.dispersion, "the Opal optics");
        assert_eq!(precious.birefringence_delta, 0.0);
    }

    let n_d = |name: &str| builtin(name).dispersion.evaluate(589.3);
    assert!((n_d("Labradorite") - 1.5625).abs() < 1e-4);
    assert!((n_d("Moonstone") - 1.523).abs() < 1e-4);
    // Labradorite: biaxial positive, n_beta nearer n_alpha; moonstone biaxial negative.
    let lab = builtin("Labradorite");
    let moon = builtin("Moonstone");
    let sign_check = |m: &GemMaterial, positive: bool| {
        let dba = m.biaxial_delta_beta_alpha.expect("biaxial");
        let gamma_minus_beta = m.birefringence_delta - dba;
        assert_eq!(dba < gamma_minus_beta, positive, "{}", m.name);
    };
    sign_check(&lab, true);
    sign_check(&moon, false);
    // The dispersion is that of a feldspar (the gemological 0.012 B-G, F-C about 0.007).
    let f_minus_c = |m: &GemMaterial| m.dispersion.evaluate(486.1) - m.dispersion.evaluate(656.3);
    assert!((f_minus_c(&lab) - 0.579 * 0.012).abs() < 2e-4);
    assert!((f_minus_c(&moon) - 0.579 * 0.012).abs() < 2e-4);

    // Black opal is darker than white opal at every wavelength. White opal has carried a faint
    // cream band at 400 nm since 2026-11-04 (its milky body), so it absorbs a little in the blue.
    let (white, black) = (builtin(names[0]), builtin(names[1]));
    for lambda in [450.0f32, 550.0, 650.0] {
        let alpha = |m: &GemMaterial| -> f32 {
            m.absorption.o_ray.iter().map(|b| b.evaluate(lambda)).sum()
        };
        assert!(
            alpha(&black) > 1.0 && alpha(&white) < 0.1 * alpha(&black),
            "{lambda}"
        );
    }
}
