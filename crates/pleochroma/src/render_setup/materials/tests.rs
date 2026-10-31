//! Tests of the render-time material resolution and override stack (`render_setup::materials`).

use super::*;

const OFF: MaterialOverrides = MaterialOverrides {
    inclusion_sigma_s: 0.0,
    c_axis_override: None,
    alpha_axis_override: None,
    edge_rounding_radius: 0.0,
    stone_width_mm: 0.0,
};

fn sized(width_mm: f32) -> MaterialOverrides {
    MaterialOverrides {
        stone_width_mm: width_mm,
        ..OFF
    }
}

/// Every `ModelUnit` built-in (all but the zoned Ametrine, which is
/// `PerMm`) and every legacy-triple color: scale `(W / 7 / K) * (2.0 / w)` for the stone width `W`
/// (7 mm while unset) and the design's model width `w` (`K = MODEL_UNIT_FACE_UP_PATH`; `None`
/// is taken as the reference width 2.0).
#[test]
fn model_unit_materials_scale_with_the_width_over_seven_mm() {
    let mut materials: Vec<GemMaterial> = GemMaterial::all_materials()
        .into_iter()
        .filter(|m| m.absorption_unit == AbsorptionUnit::ModelUnit)
        .collect();
    materials.push(GemMaterial::sapphire().with_body_color([0.2, 0.9, 0.3]));
    materials.push(GemMaterial::new_custom(
        "Fantasy",
        1.7,
        0.02,
        0.0,
        [0.1, 0.4, 0.9],
    ));
    for material in materials {
        assert_eq!(material.absorption_unit, AbsorptionUnit::ModelUnit);
        for model_width in [None, Some(1.0), Some(2.0), Some(4.0)] {
            let ratio = REFERENCE_MODEL_WIDTH / model_width.unwrap_or(REFERENCE_MODEL_WIDTH);
            let expected =
                |width_mm: f64| width_mm / 7.0 / f64::from(MODEL_UNIT_FACE_UP_PATH) * ratio;
            let close = |actual: f32, wanted: f64| {
                (f64::from(actual) - wanted).abs() <= 1e-6 * wanted.abs()
            };
            let unset = apply_material_overrides(material.clone(), &OFF, model_width);
            assert!(
                close(unset.absorption_path_scale, expected(7.0)),
                "{} at model width {model_width:?}: {}",
                material.name,
                unset.absorption_path_scale
            );
            let seven = apply_material_overrides(material.clone(), &sized(7.0), model_width);
            assert_eq!(
                seven.absorption_path_scale, unset.absorption_path_scale,
                "{}",
                material.name
            );
            let big = apply_material_overrides(material.clone(), &sized(10.87), model_width);
            assert!(
                close(big.absorption_path_scale, expected(f64::from(10.87_f32))),
                "{} at model width {model_width:?}: {}",
                material.name,
                big.absorption_path_scale
            );
        }
    }
}

/// Band colours and physics recipes are `PerMm`: `W / model_width`, 7 mm when no size is set.
#[test]
fn per_mm_materials_scale_with_the_width_over_the_model_width() {
    let banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
    assert_eq!(banded.absorption_unit, AbsorptionUnit::PerMm);
    let model_width = 2.1_f64;
    let unset = apply_material_overrides(banded.clone(), &OFF, Some(model_width));
    assert!((f64::from(unset.absorption_path_scale) - 7.0 / model_width).abs() < 1e-5);
    let sized_up = apply_material_overrides(banded.clone(), &sized(10.87), Some(model_width));
    assert!((f64::from(sized_up.absorption_path_scale) - 10.87 / model_width).abs() < 1e-5);
    // No measurable width: the material keeps the scale it has.
    let unmeasured = apply_material_overrides(banded.clone(), &sized(10.87), None);
    assert_eq!(
        unmeasured.absorption_path_scale,
        banded.absorption_path_scale
    );
    assert!(needs_model_width(&banded));
    // Every material reads the model width now: a `ModelUnit` one for the reference-width rescale.
    assert!(needs_model_width(&GemMaterial::sapphire()));
}

/// An unmeasurable arrangement leaves a `ModelUnit` material at today's calibrated look (taken
/// as the reference width), never at the raw scale 1.0 (the 2026-10-08 black stone); a degenerate
/// width (zero, negative, NaN) does the same.
#[test]
fn a_model_unit_material_without_a_measurable_width_keeps_the_reference_look() {
    for width_mm in [0.0_f32, 7.0, 10.87] {
        let reference =
            absorption_path_scale_for(AbsorptionUnit::ModelUnit, width_mm, Some(2.0)).expect("set");
        for model_width in [None, Some(0.0), Some(-1.0), Some(f64::NAN)] {
            assert_eq!(
                absorption_path_scale_for(AbsorptionUnit::ModelUnit, width_mm, model_width),
                Some(reference),
                "{width_mm} mm, model width {model_width:?}"
            );
        }
    }
}

/// The inclusion coefficient is the caller's number untouched: the size never reaches it.
#[test]
fn the_stone_size_never_changes_the_scattering_coefficient() {
    let with = |width_mm: f32| {
        apply_material_overrides(
            GemMaterial::sapphire(),
            &MaterialOverrides {
                inclusion_sigma_s: 0.2,
                stone_width_mm: width_mm,
                ..OFF
            },
            Some(2.0),
        )
    };
    assert_eq!(with(0.0).scattering_sigma_s, 0.2);
    assert_eq!(with(5.0).scattering_sigma_s, 0.2);
    assert_eq!(with(12.0).scattering_sigma_s, 0.2);
}

#[test]
fn effective_width_defaults_to_seven_mm() {
    assert_eq!(effective_stone_width_mm(0.0), 7.0);
    assert_eq!(effective_stone_width_mm(5.0), 5.0);
}

// ---- regression locks (lane AU) ----

/// The unit decision of every built-in, written out. A new material fails
/// `every_built_in_has_a_deliberate_absorption_unit` until it is added here (or to
/// `ZONED_BUILT_IN_UNITS` when it is a zoned one): choose `ModelUnit` only for
/// absorption tuned per model unit for a ~7 mm stone, `PerMm` for a physical per-millimetre
/// one.
const BUILT_IN_UNITS: &[(&str, AbsorptionUnit)] = &[
    ("Diamond", AbsorptionUnit::ModelUnit),
    ("Sapphire", AbsorptionUnit::ModelUnit),
    ("Ruby", AbsorptionUnit::ModelUnit),
    ("Emerald", AbsorptionUnit::ModelUnit),
    ("Zircon", AbsorptionUnit::ModelUnit),
    ("Alexandrite", AbsorptionUnit::ModelUnit),
    ("Topaz", AbsorptionUnit::ModelUnit),
    ("Spinel", AbsorptionUnit::ModelUnit),
    ("Quartz", AbsorptionUnit::ModelUnit),
    ("Tourmaline", AbsorptionUnit::ModelUnit),
    ("Tanzanite", AbsorptionUnit::ModelUnit),
    ("Synthetic Moissanite", AbsorptionUnit::ModelUnit),
    ("Cubic Zirconia", AbsorptionUnit::ModelUnit),
    ("Aquamarine", AbsorptionUnit::ModelUnit),
    ("Morganite", AbsorptionUnit::ModelUnit),
    ("Chrysoberyl (Yellow)", AbsorptionUnit::ModelUnit),
    ("Amethyst", AbsorptionUnit::ModelUnit),
    ("Citrine", AbsorptionUnit::ModelUnit),
    ("Pyrope Garnet", AbsorptionUnit::ModelUnit),
    ("Almandine Garnet", AbsorptionUnit::ModelUnit),
    ("Spessartine Garnet", AbsorptionUnit::ModelUnit),
    (
        "Color-Change Garnet (Pyrope-Spessartine)",
        AbsorptionUnit::ModelUnit,
    ),
    ("Grossular Garnet (Tsavorite)", AbsorptionUnit::ModelUnit),
    ("Andradite Garnet (Demantoid)", AbsorptionUnit::ModelUnit),
    ("Peridot", AbsorptionUnit::ModelUnit),
    ("YAG", AbsorptionUnit::ModelUnit),
    ("GGG", AbsorptionUnit::ModelUnit),
    ("Benitoite", AbsorptionUnit::ModelUnit),
    ("Andalusite", AbsorptionUnit::ModelUnit),
    ("Opal", AbsorptionUnit::ModelUnit),
    ("Glass (N-BK7)", AbsorptionUnit::ModelUnit),
    ("Glass (F2)", AbsorptionUnit::ModelUnit),
    ("Rutile", AbsorptionUnit::ModelUnit),
    ("Precious Opal (White)", AbsorptionUnit::ModelUnit),
    ("Precious Opal (Black)", AbsorptionUnit::ModelUnit),
    ("Labradorite", AbsorptionUnit::ModelUnit),
    ("Moonstone", AbsorptionUnit::ModelUnit),
];

/// The zoned built-ins: the zoned Ametrine is a physical, per-millimetre material (its zones
/// are chromophore recipes).
const ZONED_BUILT_IN_UNITS: &[(&str, AbsorptionUnit)] = &[("Ametrine", AbsorptionUnit::PerMm)];

/// Optical density of `material` at `lambda_nm` over `path_units` model units, as the tracer
/// computes it: summed band absorption (ordinary ray) times `absorption_path_scale` times the
/// path.
fn density(material: &GemMaterial, lambda_nm: f32, path_units: f32) -> f64 {
    let alpha: f32 = material
        .absorption
        .o_ray
        .iter()
        .map(|band| band.evaluate(lambda_nm))
        .sum();
    f64::from(alpha) * f64::from(material.absorption_path_scale) * f64::from(path_units)
}

fn transmittance(material: &GemMaterial, lambda_nm: f32, path_units: f32) -> f64 {
    (-density(material, lambda_nm, path_units)).exp()
}

#[test]
fn every_built_in_has_a_deliberate_absorption_unit() {
    let materials = GemMaterial::all_materials();
    let decisions = || BUILT_IN_UNITS.iter().chain(ZONED_BUILT_IN_UNITS);
    for material in &materials {
        let decided = decisions()
            .find(|(name, _)| *name == material.name)
            .unwrap_or_else(|| {
                panic!(
                    "built-in {} has no entry in BUILT_IN_UNITS: decide its absorption unit",
                    material.name
                )
            });
        assert_eq!(material.absorption_unit, decided.1, "{}", material.name);
    }
    assert_eq!(
        materials.len(),
        decisions().count(),
        "BUILT_IN_UNITS / ZONED_BUILT_IN_UNITS lists a material that no longer exists"
    );
}

/// For every built-in: unset and 7 mm give the identical material, 14 mm is exactly twice
/// the optical density of 7 mm, and a `ModelUnit` built-in at model width 1.0 has exactly twice
/// the scale it has at the reference width 2.0 (an unmeasurable width is the reference width).
#[test]
fn every_built_in_scales_with_size_and_reference_width() {
    for material in GemMaterial::all_materials() {
        let model_unit = material.absorption_unit == AbsorptionUnit::ModelUnit;
        // A `PerMm` built-in (the zoned Ametrine) has the scale `W / model_width` and an
        // unmeasurable width (`None`) leaves its scale alone, so the `None` case only applies
        // to a `ModelUnit` one.
        for model_width in [None, Some(1.0), Some(2.0), Some(3.7)] {
            if !model_unit && model_width.is_none() {
                continue;
            }
            let unset = apply_material_overrides(material.clone(), &OFF, model_width);
            let seven = apply_material_overrides(material.clone(), &sized(7.0), model_width);
            let fourteen = apply_material_overrides(material.clone(), &sized(14.0), model_width);
            assert_eq!(unset, seven, "{}: unset != 7 mm", material.name);
            if model_unit {
                let at = |w: Option<f64>| {
                    apply_material_overrides(material.clone(), &sized(7.0), w).absorption_path_scale
                };
                assert_eq!(at(None), at(Some(2.0)), "{}: None != 2.0", material.name);
                assert_eq!(
                    at(Some(1.0)),
                    2.0 * at(Some(2.0)),
                    "{}: width 1.0 is not twice width 2.0",
                    material.name
                );
            }
            assert_eq!(
                fourteen.absorption_path_scale,
                2.0 * seven.absorption_path_scale,
                "{}",
                material.name
            );
            for lambda in [450.0_f32, 530.0, 650.0] {
                assert_eq!(
                    density(&fourteen, lambda, 5.0),
                    2.0 * density(&seven, lambda, 5.0),
                    "{} at {lambda} nm",
                    material.name
                );
            }
        }
    }
}

fn green_preset() -> [f32; 3] {
    crate::optics::materials::body_color::BODY_COLOR_PRESETS
        .iter()
        .find(|preset| preset.label.to_ascii_lowercase().contains("green"))
        .expect("a green body-colour preset")
        .absorption_rgb
}

/// `material` with the stored (uncalibrated) scale 1.0: the "per model unit" numbers the swatch
/// is tuned on.
fn bare(material: GemMaterial) -> GemMaterial {
    material.with_absorption_path_scale(1.0)
}

/// The transmittance the swatch shows over ONE model unit at `lambda_nm`.
fn one_unit_swatch_transmittance(material: &GemMaterial, lambda_nm: f32) -> f64 {
    transmittance(&bare(material.clone()), lambda_nm, 1.0)
}

/// 2026-10-08 photo-vs-render bug: a real green cubic zirconia, 10.87 mm across the girdle,
/// is vivid green in daylight, but the render was black because the per-model-unit absorption
/// (which the swatch shows over ONE unit) was integrated over the several-unit face-up path.
///
/// Arithmetic (green preset `[2.2, 0.2, 2.0]`, the three Gaussian bands overlap so the
/// stone's alpha at 550 nm is about 1.34 per unit, not 0.2): the 1-unit swatch has
/// `T1 = exp(-1.34) = 0.26`. The scale at 10.87 mm on a design of the reference width 2.0 is
/// `10.87 / 7 / K` (K = 2.52, but the result does not depend on K), so over K units the density is
/// `1.34 * 10.87 / 7 = 2.08` and `T = T1^(10.87/7) = 0.125`; a design of model width `w` raises
/// that exponent by `2.0 / w` (the same physical stone, so a smaller model is a longer path per
/// model unit). The old behaviour (scale `10.87 / 7 = 1.55` over the same
/// K units) had density `1.34 * 1.55 * K`: 3.3e-5 at the original estimate K = 5 (density
/// 10.4), 5e-3 at K = 2.52. The floor 0.05 is 2.5 times below the new value and 10 times
/// above the old one at the measured K (still above it at model width 1.8, exponent 1.73).
#[test]
fn regression_green_cubic_zirconia_at_10_87_mm_is_not_black() {
    let green = GemMaterial::by_name("Cubic Zirconia")
        .expect("built-in")
        .with_body_color(green_preset());
    let t1 = one_unit_swatch_transmittance(&green, 550.0);
    for model_width in [1.8, 2.0, 2.2] {
        let expected = t1.powf(f64::from(10.87_f32 / 7.0) * REFERENCE_MODEL_WIDTH / model_width);
        let stone = apply_material_overrides(green.clone(), &sized(10.87), Some(model_width));
        let t_green = transmittance(&stone, 550.0, MODEL_UNIT_FACE_UP_PATH);
        assert!(
            (t_green / expected - 1.0).abs() < 1e-3,
            "model width {model_width}: green T {t_green} vs analytic {expected}"
        );
        assert!(
            t_green > 0.05,
            "model width {model_width}: green T {t_green} (old behaviour: 3e-5)"
        );
        assert!(t_green > transmittance(&stone, 450.0, MODEL_UNIT_FACE_UP_PATH));
        assert!(t_green > transmittance(&stone, 650.0, MODEL_UNIT_FACE_UP_PATH));
    }
}

/// Same lock for plain Emerald and Ruby at 10.87 mm: the face-up transmittance equals the
/// 1-unit swatch raised to `10.87 / 7` and the own colour channel stays above the opposite one.
///
/// Emerald (alpha at 530 nm about 0.57 per unit): new `T = exp(-0.57 * 1.553) = 0.41`, old
/// (scale 1.553 over K units) `exp(-0.885 * K)`: 0.012 at the original estimate K = 5, 0.107
/// at the measured K = 2.52; the floor 0.2 is half the new value and 1.9 times the old one at
/// the measured K. Ruby has no separate absolute floor beyond 0.05: its swatch T is
/// already high, and the analytic equality is the real check.
#[test]
fn regression_emerald_and_ruby_at_10_87_mm_stay_coloured() {
    let factor = f64::from(10.87_f32 / 7.0);
    let emerald = apply_material_overrides(GemMaterial::emerald(), &sized(10.87), Some(2.0));
    let green = transmittance(&emerald, 530.0, MODEL_UNIT_FACE_UP_PATH);
    let expected = one_unit_swatch_transmittance(&GemMaterial::emerald(), 530.0).powf(factor);
    assert!(
        (green / expected - 1.0).abs() < 1e-3,
        "emerald {green} vs {expected}"
    );
    assert!(
        green > 0.2,
        "emerald green T {green} (old behaviour: 0.012)"
    );
    assert!(green > 1.5 * transmittance(&emerald, 650.0, MODEL_UNIT_FACE_UP_PATH));
    let ruby = apply_material_overrides(GemMaterial::ruby(), &sized(10.87), Some(2.0));
    let red = transmittance(&ruby, 650.0, MODEL_UNIT_FACE_UP_PATH);
    let expected = one_unit_swatch_transmittance(&GemMaterial::ruby(), 650.0).powf(factor);
    assert!(
        (red / expected - 1.0).abs() < 1e-3,
        "ruby {red} vs {expected}"
    );
    assert!(red > 0.05, "ruby red T {red}");
    assert!(red > 1.5 * transmittance(&ruby, 530.0, MODEL_UNIT_FACE_UP_PATH));
}

/// The face-up path, in model units, of a stone whose design has the model width `model_width`:
/// `K` model units for the reference-width stone, scaling with the stone's size in model space.
fn face_up_path_units(model_width: f64) -> f64 {
    f64::from(MODEL_UNIT_FACE_UP_PATH) * model_width / REFERENCE_MODEL_WIDTH
}

/// The calibration itself: unset size and 7 mm render, over `K` units of path, exactly the
/// 1-unit swatch transmittance, for every `ModelUnit` built-in (not the per-millimetre
/// Ametrine) and for every toolbar color preset, on a reference-width design.
#[test]
fn unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch() {
    unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch_at(2.0);
}

/// The same calibration on a design normalised twice as large: the stone is twice as large in
/// model space, so its face-up path is `K * 2` model units and the colors are unchanged.
#[test]
fn unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch_at_double_model_width() {
    unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch_at(4.0);
}

fn unset_and_seven_mm_face_up_path_renders_the_one_unit_swatch_at(model_width: f64) {
    use crate::optics::materials::body_color::BODY_COLOR_PRESETS;
    let mut materials: Vec<GemMaterial> = GemMaterial::all_materials()
        .into_iter()
        .filter(|m| m.absorption_unit == AbsorptionUnit::ModelUnit)
        .collect();
    for preset in &BODY_COLOR_PRESETS {
        materials.push(
            GemMaterial::by_name("Cubic Zirconia")
                .expect("built-in")
                .with_body_color(preset.absorption_rgb),
        );
    }
    for material in materials {
        assert_eq!(material.absorption_unit, AbsorptionUnit::ModelUnit);
        for overrides in [OFF, sized(7.0)] {
            let stone = apply_material_overrides(material.clone(), &overrides, Some(model_width));
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a path of a few model units is far inside f32"
            )]
            let path_units = face_up_path_units(model_width) as f32;
            for lambda in [420.0_f32, 450.0, 500.0, 530.0, 580.0, 620.0, 650.0, 700.0] {
                let face_up = density(&stone, lambda, path_units);
                let swatch = density(&bare(material.clone()), lambda, 1.0);
                assert!(
                    (face_up - swatch).abs() <= 1e-5 * swatch.max(1.0),
                    "{} at {lambda} nm: face-up density {face_up} vs swatch {swatch}",
                    material.name
                );
            }
        }
    }
}

/// Swatch == render at 5, 7 and 10.87 mm: the color of the 1-unit absorption over the swatch
/// path `width / 7` (what the swatch shows) is the color of the sized material over
/// `K` face-up units (what the render integrates), for every preset, on a reference-width design.
#[test]
fn the_swatch_equals_the_render_material_at_every_size() {
    the_swatch_equals_the_render_material_at_every_size_for(2.0);
}

/// The same on a design normalised twice as large: the render path is `K * 2` model units.
#[test]
fn the_swatch_equals_the_render_material_at_every_size_at_double_model_width() {
    the_swatch_equals_the_render_material_at_every_size_for(4.0);
}

fn the_swatch_equals_the_render_material_at_every_size_for(model_width: f64) {
    use crate::{
        color::body_color::{Illuminant, body_colors},
        optics::materials::body_color::BODY_COLOR_PRESETS,
    };
    for preset in &BODY_COLOR_PRESETS {
        let material = GemMaterial::by_name("Cubic Zirconia")
            .expect("built-in")
            .with_body_color(preset.absorption_rgb);
        for width_mm in [5.0_f32, 7.0, 10.87] {
            let stone =
                apply_material_overrides(material.clone(), &sized(width_mm), Some(model_width));
            let render = body_colors(
                &stone.absorption,
                face_up_path_units(model_width) * f64::from(stone.absorption_path_scale),
                Illuminant::D65,
            )
            .unpolarised;
            let swatch = body_colors(
                &bare(material.clone()).absorption,
                f64::from(width_mm) / 7.0,
                Illuminant::D65,
            )
            .unpolarised;
            for (r, s) in render.lab.iter().zip(swatch.lab) {
                assert!(
                    (r - s).abs() < 1e-3,
                    "{} at {width_mm} mm: render {:?} vs swatch {:?}",
                    preset.label,
                    render.lab,
                    swatch.lab
                );
            }
        }
    }
}

/// Per-mm materials are physically calibrated: the face-up constant never reaches them.
#[test]
fn the_calibration_constant_never_reaches_per_mm_materials() {
    for (width_mm, model_width) in [(0.0_f32, 2.0_f64), (7.0, 2.0), (10.87, 2.0), (10.87, 3.5)] {
        let scale = absorption_path_scale_for(AbsorptionUnit::PerMm, width_mm, Some(model_width))
            .expect("a measurable width");
        let expected = f64::from(effective_stone_width_mm(width_mm)) / model_width;
        assert!((f64::from(scale) - expected).abs() < 1e-5, "{width_mm} mm");
    }
}

/// Calibration of [`MODEL_UNIT_FACE_UP_PATH`]: the measured face-up `mean_path_units` of the
/// standard round brilliant and the emerald cut (colourless stone, light tent, table-up), each
/// rescaled by `REFERENCE_MODEL_WIDTH / its model width` to the units of a reference-width stone,
/// must be within +-20 % of the constant at their median, and the single path whose color matches
/// the Beer-Lambert mixture of a green stone (the mixture is lighter than the mean path) must
/// be of the same order. The failure message prints every measured value so the owner can
/// re-pin the constant.
#[test]
fn model_unit_face_up_path_matches_the_measured_cuts() {
    use crate::{
        color::{
            body_color::{Illuminant, body_colors},
            metrics::evaluate_gem_optical_metrics_with_tone,
        },
        geometry::cuts::StandardGemCuts,
        optics::raytracer::LightingPreset,
        render_setup::measure_model_width,
    };
    use std::{f32::consts::FRAC_PI_2, fmt::Write as _};

    let environment = LightingPreset::LightTent.studio(1.0, 0.85, 0.95);
    let cuts = [
        (
            "round brilliant",
            StandardGemCuts::standard_round_brilliant(),
        ),
        ("emerald cut", StandardGemCuts::emerald_cut()),
    ];
    let green = bare(
        GemMaterial::by_name("Cubic Zirconia")
            .expect("built-in")
            .with_body_color(green_preset()),
    );
    let mut mean_paths = Vec::new();
    let mut equivalent_paths = Vec::new();
    let mut report = String::new();
    for (name, planes) in &cuts {
        // `mean_path_units` is independent of the absorption (the path histogram is collected
        // before any colour), so the green stone gives the colourless stone's path too.
        let (_, tone) =
            evaluate_gem_optical_metrics_with_tone(planes, &green, 0.0, FRAC_PI_2, environment);
        // The single path (model units) whose D65 lightness equals the mixture's, by bisection
        // (lightness falls monotonically with the path).
        let (mut low, mut high) = (0.01_f64, 80.0_f64);
        for _ in 0..60 {
            let mid = f64::midpoint(low, high);
            let l_star = body_colors(&green.absorption, mid, Illuminant::D65)
                .unpolarised
                .lab[0];
            if l_star > f64::from(tone.l_star) {
                low = mid;
            } else {
                high = mid;
            }
        }
        // Both paths are in the cut's own model units; the constant is in units of a
        // `REFERENCE_MODEL_WIDTH`-wide stone, so rescale by `REFERENCE_MODEL_WIDTH / width`.
        let rescale = REFERENCE_MODEL_WIDTH
            / measure_model_width(planes).expect("a standard cut has a measurable width");
        let equivalent = f64::midpoint(low, high) * rescale;
        let mean_path = f64::from(tone.mean_path_units) * rescale;
        let _ = write!(
            report,
            "{name}: mean_path_units {mean_path:.3} (rescale {rescale:.4}), \
             mixture-equivalent path {equivalent:.2}; ",
        );
        mean_paths.push(mean_path);
        equivalent_paths.push(equivalent);
    }
    let median = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        let mid = values.len() / 2;
        if values.len().is_multiple_of(2) {
            f64::midpoint(values[mid - 1], values[mid])
        } else {
            values[mid]
        }
    };
    let mean_median = median(&mut mean_paths);
    let equivalent_median = median(&mut equivalent_paths);
    let constant = f64::from(MODEL_UNIT_FACE_UP_PATH);
    // The owner re-pins the constant from this line (`--nocapture`).
    println!(
        "MODEL_UNIT_FACE_UP_PATH {constant}: {report}mean median {mean_median:.3}, \
         equivalent median {equivalent_median:.3}"
    );
    assert!(
        (constant / mean_median - 1.0).abs() <= 0.20,
        "MODEL_UNIT_FACE_UP_PATH {constant} is not within 20 % of the median mean path \
             {mean_median:.2} ({report}equivalent median {equivalent_median:.2}): re-pin it"
    );
    assert!(
        constant > 0.4 * equivalent_median && constant < 2.5 * equivalent_median,
        "MODEL_UNIT_FACE_UP_PATH {constant} is far from the mixture-equivalent path \
             {equivalent_median:.2} ({report}mean median {mean_median:.2})"
    );
}

/// The standard round brilliant with every plane offset multiplied by `q` (normals unchanged):
/// the same stone normalised `q` times as large, its measured model width `q` times as large.
fn round_brilliant_scaled(q: f32) -> Vec<GpuFacetPlane> {
    crate::geometry::cuts::StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(|plane| GpuFacetPlane {
            normal: plane.normal,
            d: plane.d * q,
        })
        .collect()
}

/// One material of every kind that matters: a built-in `ModelUnit`, a toolbar color preset on
/// Cubic Zirconia (`ModelUnit`), and a band color (`PerMm`).
fn scale_test_materials() -> Vec<GemMaterial> {
    let materials = vec![
        GemMaterial::emerald(),
        GemMaterial::by_name("Cubic Zirconia")
            .expect("built-in")
            .with_body_color(green_preset()),
        GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0),
    ];
    assert_eq!(materials[0].absorption_unit, AbsorptionUnit::ModelUnit);
    assert_eq!(materials[1].absorption_unit, AbsorptionUnit::ModelUnit);
    assert_eq!(materials[2].absorption_unit, AbsorptionUnit::PerMm);
    materials
}

/// The same physical stone normalised at model scales 0.5, 1 and 2 has the same optical depth
/// along the same physical ray: a model-space path of `q * p` units in the `q`-scaled design
/// is the path of `p` units in the original, and must integrate to the same absorption.
#[test]
fn equal_physical_stones_at_model_scales_half_one_two_have_equal_optical_depth() {
    let widths: Vec<(f32, f64)> = [0.5_f32, 1.0, 2.0]
        .into_iter()
        .map(|q| {
            let width = measure_model_width(&round_brilliant_scaled(q)).expect("measurable");
            (q, width)
        })
        .collect();
    let (_, width_one) = widths[1];
    for material in scale_test_materials() {
        for width_mm in [7.0_f32, 10.87] {
            let reference =
                apply_material_overrides(material.clone(), &sized(width_mm), Some(width_one));
            for &(q, width_q) in &widths {
                let scaled =
                    apply_material_overrides(material.clone(), &sized(width_mm), Some(width_q));
                for lambda in [450.0_f32, 530.0, 650.0] {
                    let path = MODEL_UNIT_FACE_UP_PATH;
                    let expected = density(&reference, lambda, path);
                    let actual = density(&scaled, lambda, q * path);
                    assert!(
                        (actual - expected).abs() <= 1e-6 * expected.abs().max(1e-12),
                        "{} at {width_mm} mm, model scale {q}, {lambda} nm: {actual} vs {expected}",
                        material.name
                    );
                }
            }
        }
    }
}

/// Doubling the physical width doubles the optical depth at every model scale, for `ModelUnit`
/// and `PerMm` materials alike.
#[test]
fn doubling_physical_width_doubles_optical_depth_at_every_model_scale() {
    for material in scale_test_materials() {
        for model_width in [1.0_f64, 2.0, 4.0] {
            let seven = apply_material_overrides(material.clone(), &sized(7.0), Some(model_width));
            let fourteen =
                apply_material_overrides(material.clone(), &sized(14.0), Some(model_width));
            for lambda in [450.0_f32, 530.0, 650.0] {
                let one = density(&seven, lambda, MODEL_UNIT_FACE_UP_PATH);
                let two = density(&fourteen, lambda, MODEL_UNIT_FACE_UP_PATH);
                assert!(
                    (-2.0_f64).mul_add(one, two).abs() <= 1e-6 * one.abs().max(1e-12),
                    "{} at model width {model_width}, {lambda} nm: {two} vs 2 x {one}",
                    material.name
                );
            }
        }
    }
}

/// Prints the measured model width of each bundled `.asc` fixture, so the owner can see how far
/// real designs sit from the reference width 2.0 (and so how far their colored renders move).
/// Run with `--ignored --nocapture`; the fixtures are listed, not walked.
#[test]
#[ignore = "prints the model widths of the bundled .asc fixtures; run with --ignored --nocapture"]
fn print_model_widths_of_bundled_asc_fixtures() {
    use crate::geometry::cuts::StandardGemCuts;
    use pleochroma_formats::asc::parse_asc;
    let fixtures = [
        (
            "trichecker12",
            include_str!("../../../tests/fixtures/asc/trichecker12.asc"),
        ),
        (
            "forfun",
            include_str!("../../../tests/fixtures/asc/forfun.asc"),
        ),
        (
            "texasstar",
            include_str!("../../../tests/fixtures/asc/texasstar.asc"),
        ),
        ("shah", include_str!("../../../tests/fixtures/asc/shah.asc")),
    ];
    for (name, content) in fixtures {
        let schedule = parse_asc(content).expect("the bundled fixture parses");
        let planes = StandardGemCuts::from_asc_schedule(&schedule);
        let width = measure_model_width(&planes);
        let ratio = width.map(|w| REFERENCE_MODEL_WIDTH / w);
        println!("{name}: model width {width:?}, ModelUnit rescale {ratio:?}");
    }
}

/// B5: a stone sized for scoring outside the render loop (`material_for_stone`: tone,
/// tilt, comparisons, the optimizer) is the very material the render uses, for a built-in,
/// a legacy triple and a per-mm material.
#[test]
fn the_scoring_material_equals_the_render_material_for_a_sized_stone() {
    use crate::{geometry::cuts::StandardGemCuts, render_setup::measure_model_width};
    let planes = StandardGemCuts::standard_round_brilliant();
    let width = measure_model_width(&planes);
    let materials = [
        GemMaterial::emerald(),
        GemMaterial::sapphire().with_body_color([0.2, 0.9, 0.3]),
        GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0),
    ];
    for material in materials {
        for width_mm in [0.0_f32, 7.0, 10.87] {
            assert_eq!(
                material_for_stone(material.clone(), width_mm, &planes),
                apply_material_overrides(material.clone(), &sized(width_mm), width),
                "{} at {width_mm} mm",
                material.name
            );
        }
    }
}

/// Per-mm materials: scale is `W / model_width`, 7 mm while unset, whatever made the bands
/// (L*C*h or library band rows, a physics recipe tensor).
#[test]
fn per_mm_materials_scale_with_width_over_model_width_whatever_made_them() {
    let bands = GemMaterial::sapphire().with_body_color_bands(&[[460.0, 45.0, 0.25]], 3.5);
    let physics = GemMaterial::sapphire().with_chromophore_absorption(bands.absorption.clone());
    for material in [bands, physics] {
        assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
        for model_width in [1.0_f64, 2.0, 3.7] {
            let unset = apply_material_overrides(material.clone(), &OFF, Some(model_width));
            assert!((f64::from(unset.absorption_path_scale) - 7.0 / model_width).abs() < 1e-5);
            let big = apply_material_overrides(material.clone(), &sized(10.87), Some(model_width));
            let expected = f64::from(10.87_f32) / model_width;
            assert!((f64::from(big.absorption_path_scale) - expected).abs() < 1e-5);
        }
    }
}

/// The zoned built-in is `PerMm`: the scale is `W / model_width` (7 mm while unset), and the
/// size step leaves the zones exactly as they were.
#[test]
fn the_zoned_built_in_scales_with_the_width_over_the_model_width() {
    let ametrine = GemMaterial::ametrine();
    assert!(needs_model_width(&ametrine));
    let unset = apply_material_overrides(ametrine.clone(), &OFF, Some(2.0));
    assert!(
        (unset.absorption_path_scale - 3.5).abs() < 1e-6,
        "7 mm / 2.0"
    );
    let ten = apply_material_overrides(ametrine, &sized(10.0), Some(2.0));
    assert!(
        (ten.absorption_path_scale - 5.0).abs() < 1e-6,
        "10 mm / 2.0"
    );
    let zoning = crate::optics::materials::ametrine_zoning();
    assert_eq!(unset.zoning.as_ref(), Some(&zoning));
    assert_eq!(ten.zoning.as_ref(), Some(&zoning));
}

/// A custom material's families (resolved in its stored orientation) follow a design's own Crystal
/// Axis; a design that sets none leaves them as they are.
#[test]
fn custom_families_follow_the_design_crystal_axis() {
    use crate::optics::inclusions::{CrystalDir, CrystalFrame, Family, FamilyKind};
    let family = Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    };
    let mut material = GemMaterial::sapphire();
    material.c_axis = Vec3::Y;
    let material = material.with_inclusions(CrystalFrame::new(Vec3::Y, None).resolve(&[family]));

    let untouched = apply_material_overrides(material.clone(), &OFF, None);
    assert_eq!(untouched.inclusions, material.inclusions);

    let overrides = MaterialOverrides {
        c_axis_override: Some(Vec3::X),
        ..OFF
    };
    let turned = apply_material_overrides(material, &overrides, None);
    let needle = turned.oriented_inclusions().expect("silk").families()[0].dir;
    assert!((needle - Vec3::X).length() < 1e-5, "{needle:?}");
}

/// A custom labradorite's lamella normal turns with the design's Crystal Axis like a family
/// direction does (it was copied unchanged before, with the families alone rebased).
#[test]
fn a_custom_lamella_normal_follows_the_design_crystal_axis() {
    use crate::optics::inclusions::bragg::{LABRADORITE_NORMAL, StructureKind, StructureSpec};
    let spec = StructureSpec::default_for("Labradorite").expect("labradorite default");
    let mut material = GemMaterial::sapphire();
    material.c_axis = Vec3::Y;
    let material = material.with_structure_spec_as(StructureKind::Labradorite, &spec);
    let before = material
        .oriented_inclusions()
        .expect("set")
        .structure()
        .expect("structure");
    let stored = CrystalFrame::new(Vec3::Y, None).direction(LABRADORITE_NORMAL);
    assert!((before.normal - stored).length() < 1e-5, "stored frame");

    let overrides = MaterialOverrides {
        c_axis_override: Some(Vec3::X),
        ..OFF
    };
    let turned = apply_material_overrides(material, &overrides, None);
    let normal = turned
        .oriented_inclusions()
        .and_then(|set| set.structure())
        .expect("structure")
        .normal;
    let expected = CrystalFrame::new(Vec3::X, None).direction(LABRADORITE_NORMAL);
    assert!(
        (normal - expected).length() < 1e-5,
        "{normal:?} vs {expected:?}"
    );
}

/// The built-in Ametrine's sector boundary follows the design's Crystal Axis: with the axis in
/// the girdle plane the boundary still runs straight down through the table, along the axis line
/// (it stayed the fixed `x = 0` plane before, whatever the axis).
#[test]
fn the_ametrine_boundary_follows_the_design_crystal_axis() {
    use glam::DVec3;
    let ametrine = GemMaterial::ametrine();
    let stored = ametrine.zoning.as_ref().expect("zoned").frame;
    assert_eq!(stored, ZoneFrame::IDENTITY, "through the table by default");
    assert_eq!(
        apply_material_overrides(ametrine.clone(), &OFF, None)
            .zoning
            .expect("zoned")
            .frame,
        ZoneFrame::IDENTITY
    );
    for axis in [Vec3::X, Vec3::Z, Vec3::new(0.6, 0.0, 0.8)] {
        let overrides = MaterialOverrides {
            c_axis_override: Some(axis),
            ..OFF
        };
        let frame = apply_material_overrides(ametrine.clone(), &overrides, None)
            .zoning
            .expect("zoned")
            .frame;
        let axis = axis.as_dvec3().normalize();
        assert!(
            frame.direction(DVec3::Y).dot(axis).abs() > 1.0 - 1e-9,
            "{axis:?}"
        );
        let normal = frame.direction(DVec3::X);
        assert!(
            normal.y.abs() < 1e-12,
            "boundary through the table: {normal:?}"
        );
        assert!(normal.dot(axis).abs() < 1e-9, "boundary contains the axis");
    }
}
