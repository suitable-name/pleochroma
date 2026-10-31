//! Tests for the text forms of a custom material's render-physics data.

use super::*;
use pleochroma::optics::inclusions::{
    CrystalDir, FamilyKind,
    bragg::{DEFAULT_SEED, StructurePattern},
};

fn cauchy(a: f32, b: f32) -> DispersionModel {
    DispersionModel::Cauchy { a, b, c: 0.0 }
}

/// The beta curve of the biaxial sample below: `1.72 + 0.0105 / lambda^2`.
fn beta_curve() -> DispersionModel {
    cauchy(1.72, 0.0105)
}

/// A biaxial custom material whose own (beta) curve is [`beta_curve`].
fn biaxial_sample() -> GemMaterial {
    let mut material = GemMaterial::new_custom("Biaxial Sample", 1.75, 0.02, 0.01, [0.0; 3]);
    material.optical_character = OpticalCharacter::BiaxialPositive;
    material.biaxial_delta_beta_alpha = Some(0.004);
    material.dispersion = beta_curve();
    material
}

fn needle(sigma: f32) -> Family {
    Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::Basal { turn_deg: 60.0 },
        sigma,
        p: 2,
        lobe: 0.06,
        rho: 0.5,
    }
}

fn structure() -> StructureSpec {
    StructureSpec {
        pattern: StructurePattern::Harlequin,
        seed: DEFAULT_SEED,
        size: Some(0.3),
        color_low_nm: 440.0,
        color_high_nm: 680.0,
        purity: 0.05,
        coverage: 0.8,
    }
}

#[test]
fn per_axis_curves_round_trip_in_both_shapes() {
    let uniaxial = PerAxisDispersion::Uniaxial(cauchy(1.76, 0.006));
    let text = per_axis_dispersion_to_json(&uniaxial).expect("writes");
    assert!(text.starts_with(r#"{"e":{"kind":"cauchy""#), "{text}");
    assert_eq!(per_axis_dispersion_from_json(&text), Some(uniaxial));

    let biaxial = PerAxisDispersion::Biaxial([cauchy(1.74, 0.005), cauchy(1.75, 0.007)]);
    let text = per_axis_dispersion_to_json(&biaxial).expect("writes");
    assert!(
        text.contains(r#""alpha":"#) && text.contains(r#""gamma":"#),
        "{text}"
    );
    assert!(!text.contains(r#""e":"#), "{text}");
    assert_eq!(per_axis_dispersion_from_json(&text), Some(biaxial));
}

#[test]
fn ordered_custom_curves_are_applied() {
    // alpha = beta - 0.01 and gamma = beta + 0.02 at every wavelength (flat offsets).
    let pair = [cauchy(1.71, 0.0105), cauchy(1.74, 0.0105)];
    assert!(biaxial_curves_are_ordered(
        &beta_curve(),
        &pair[0],
        &pair[1]
    ));
    let material = with_custom_physics(
        biaxial_sample(),
        Some(PerAxisDispersion::Biaxial(pair)),
        None,
        None,
    );
    assert_eq!(material.biaxial_principal_dispersion, Some(pair));
}

#[test]
fn crossing_custom_curves_are_ignored() {
    // `1.75 + 0.0005 / l^2` minus the beta curve is `0.03 - 0.01 / l^2`: negative at 400 nm,
    // positive at 700 nm, so alpha starts below beta and ends above it.
    let crossing = cauchy(1.75, 0.0005);
    let beta = beta_curve();
    assert!(crossing.evaluate(400.0) < beta.evaluate(400.0));
    assert!(crossing.evaluate(700.0) > beta.evaluate(700.0));

    let pair = [crossing, cauchy(1.74, 0.0105)];
    let untouched = biaxial_sample();
    let material = with_custom_physics(
        untouched.clone(),
        Some(PerAxisDispersion::Biaxial(pair)),
        None,
        None,
    );
    assert_eq!(material.biaxial_principal_dispersion, None);
    // Nothing else moved: the achromatic spreads keep applying.
    assert_eq!(material, untouched);
}

#[test]
fn curves_touching_the_beta_curve_are_ignored() {
    let pair = [beta_curve(), cauchy(1.74, 0.0105)];
    assert!(!biaxial_curves_are_ordered(
        &beta_curve(),
        &pair[0],
        &pair[1]
    ));
    let untouched = biaxial_sample();
    let material = with_custom_physics(
        untouched.clone(),
        Some(PerAxisDispersion::Biaxial(pair)),
        None,
        None,
    );
    assert_eq!(material.biaxial_principal_dispersion, None);
    assert_eq!(material, untouched);

    // The same for gamma equal to beta.
    let pair = [cauchy(1.71, 0.0105), beta_curve()];
    assert!(!biaxial_curves_are_ordered(
        &beta_curve(),
        &pair[0],
        &pair[1]
    ));
}

#[test]
fn biaxial_curves_are_ordered_samples_the_whole_band() {
    // `1.69 + 0.016 / l^2` minus the beta curve is `-0.03 + 0.0055 / l^2`: below beta at the D
    // line (where a one-point check passes), above it at 400 nm.
    let beta = beta_curve();
    let late_crossing = cauchy(1.69, 0.016);
    assert!(late_crossing.evaluate(589.3) < beta.evaluate(589.3));
    assert!(late_crossing.evaluate(400.0) > beta.evaluate(400.0));
    let gamma = cauchy(1.74, 0.0105);
    assert!(!biaxial_curves_are_ordered(&beta, &late_crossing, &gamma));

    // The mirror image at the red end: `1.70 + 0.0205 / l^2` minus the beta curve is
    // `-0.02 + 0.01 / l^2`, above beta at the D line and below it at 780 nm.
    let early = cauchy(1.71, 0.0105);
    let red_crossing = cauchy(1.70, 0.0205);
    assert!(red_crossing.evaluate(589.3) > beta.evaluate(589.3));
    assert!(red_crossing.evaluate(780.0) < beta.evaluate(780.0));
    assert!(!biaxial_curves_are_ordered(&beta, &early, &red_crossing));

    // A clean pair passes; the same two curves swapped do not.
    assert!(biaxial_curves_are_ordered(&beta, &early, &gamma));
    assert!(!biaxial_curves_are_ordered(&beta, &gamma, &early));
}

#[test]
fn unusable_per_axis_text_reads_as_none() {
    for text in [
        "",
        "   ",
        "not json",
        "{}",
        // A half-set biaxial pair, or an `e` curve beside one, is not a material.
        r#"{"alpha":{"kind":"cauchy","a":1.7,"b":0.005,"c":0.0}}"#,
        r#"{"e":{"kind":"cauchy","a":1.7,"b":0.005,"c":0.0},"gamma":{"kind":"cauchy","a":1.7,"b":0.005,"c":0.0}}"#,
        // A curve the renderer refuses (an index below 1).
        r#"{"e":{"kind":"cauchy","a":0.5,"b":0.0,"c":0.0}}"#,
        // The placeholder the column's earlier tests stored.
        r#"{"kind":"uniaxial_extraordinary","a":1.7,"b":0.01}"#,
    ] {
        assert_eq!(per_axis_dispersion_from_json(text), None, "{text:?}");
    }
}

#[test]
fn inclusion_families_round_trip_and_cap_at_the_family_limit() {
    let families = vec![
        needle(0.1),
        Family {
            kind: FamilyKind::Platelet,
            dir: CrystalDir::AlongC,
            sigma: 0.5,
            p: 1,
            lobe: 0.25,
            rho: 0.0,
        },
    ];
    let text = inclusion_families_to_json(&families).expect("writes");
    assert_eq!(inclusion_families_from_json(&text), Some(families));

    assert_eq!(inclusion_families_to_json(&[]), None);

    let many = vec![needle(0.1); MAX_FAMILIES + 2];
    let text = inclusion_families_to_json(&many).expect("writes");
    assert_eq!(
        inclusion_families_from_json(&text).map(|f| f.len()),
        Some(MAX_FAMILIES)
    );
}

#[test]
fn unusable_inclusion_text_reads_as_none() {
    for text in ["", "[]", "not json", r#"{"kind":"Needle"}"#] {
        assert_eq!(inclusion_families_from_json(text), None, "{text:?}");
    }
    // Families that cannot scatter anything are not a set.
    let silent = serde_json::to_string(&[needle(0.0), needle(-1.0)]).unwrap();
    assert_eq!(inclusion_families_from_json(&silent), None);
}

#[test]
fn a_structure_choice_round_trips_and_a_broken_one_reads_as_none() {
    let spec = structure();
    let text = structure_spec_to_json(&spec).expect("writes");
    assert_eq!(structure_spec_from_json(&text), Some(spec));

    for text in ["", "not json", "{}"] {
        assert_eq!(structure_spec_from_json(text), None, "{text:?}");
    }
    let backwards = StructureSpec {
        color_low_nm: 700.0,
        color_high_nm: 500.0,
        ..structure()
    };
    let text = structure_spec_to_json(&backwards).expect("writes");
    assert_eq!(structure_spec_from_json(&text), None);
}

#[test]
fn a_structure_kind_rides_in_the_text_and_a_kindless_text_is_unchanged() {
    let kindless = CustomStructure {
        kind: None,
        spec: structure(),
    };
    let text = custom_structure_to_json(&kindless).expect("writes");
    assert_eq!(Some(text.clone()), structure_spec_to_json(&structure()));
    assert_eq!(custom_structure_from_json(&text), Some(kindless));

    let opal = CustomStructure {
        kind: Some(StructureKind::Opal),
        spec: structure(),
    };
    let text = custom_structure_to_json(&opal).expect("writes");
    assert!(text.contains(r#""kind":"opal""#), "{text}");
    assert_eq!(custom_structure_from_json(&text), Some(opal));
    // The spec reader still accepts the text, ignoring the kind.
    assert_eq!(structure_spec_from_json(&text), Some(structure()));

    let unknown = text.replace("opal", "quartz");
    assert_eq!(
        custom_structure_from_json(&unknown).map(|s| s.kind),
        Some(None)
    );
}

#[test]
fn only_a_structure_that_names_its_kind_is_applied() {
    let plain = GemMaterial::new_custom("Sample", 1.7, 0.02, -0.008, [0.0; 3]);
    let kindless = CustomStructure {
        kind: None,
        spec: structure(),
    };
    assert_eq!(with_custom_structure(plain.clone(), Some(&kindless)), plain);
    assert_eq!(with_custom_structure(plain.clone(), None), plain);
    let opal = CustomStructure {
        kind: Some(StructureKind::Opal),
        spec: structure(),
    };
    assert_ne!(with_custom_structure(plain.clone(), Some(&opal)), plain);
}
