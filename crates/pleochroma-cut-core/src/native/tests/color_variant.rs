//! A design's color variant (`MaterialSelection::color_variant_override`) in the `.psdd` file:
//! the `[material.color_variant]` table, the legacy fallback triple written next to it, and the
//! unchanged read of a file without one.

use super::fixtures::simple_design;
use crate::{
    material::{MaterialSelection, VariantChoice, color::nearest_legacy_rgb},
    native::{
        DesignExtras, DesignLoadError, convert, design_from_file, design_from_str, design_to_file,
        design_to_string,
    },
};
use pleochroma::{
    color::body_color::{Illuminant, body_colors},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{
            AbsorptionUnit, GemMaterial,
            variants::{MAX_VARIANT_BANDS, face_up_swatch, face_up_swatch_path},
        },
    },
};

/// A uniaxial variant with a mix of wavelength and energy bands, none of its values a round
/// binary fraction (so a lossy text round trip would show).
fn uniaxial_choice() -> VariantChoice {
    VariantChoice {
        key: "sapphire.test_blue".to_owned(),
        label: "Test Blue".to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![
                AbsorptionBand::energy(580.0, 3100.3, 0.4321),
                AbsorptionBand::new(450.0, 37.7, 0.0123),
            ],
            vec![
                AbsorptionBand::new(600.0, 51.1, 0.2),
                AbsorptionBand::energy(500.0, 2500.7, 0.37),
                AbsorptionBand::energy(420.0, 1800.9, 0.9137),
            ],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 5.5,
    }
}

/// A trichroic (alpha / beta / gamma) variant, per model unit.
fn biaxial_choice() -> VariantChoice {
    VariantChoice {
        key: "user.0a1b2c3d".to_owned(),
        label: "Test Trichroic".to_owned(),
        absorption: AbsorptionTensor::biaxial(
            vec![AbsorptionBand::new(640.0, 44.4, 0.31)],
            vec![AbsorptionBand::energy(540.0, 2200.2, 0.17)],
            vec![
                AbsorptionBand::new(460.0, 39.9, 0.5555),
                AbsorptionBand::energy(700.0, 1500.1, 0.0421),
            ],
        ),
        unit: AbsorptionUnit::ModelUnit,
        reference_path_mm: 7.0,
    }
}

/// The fixture design with `choice` as its color variant.
fn design_with(choice: &VariantChoice) -> crate::design::Design {
    let mut design = simple_design();
    design.material.name = Some("Sapphire".to_owned());
    design.material = design.material.with_color_variant(Some(choice.clone()));
    design
}

/// `design` written and opened again.
fn round_trip(design: &crate::design::Design) -> MaterialSelection {
    let text = design_to_string(design, None, &DesignExtras::default()).expect("must save");
    design_from_str(&text)
        .expect("must load back")
        .design
        .material
}

#[test]
fn a_uniaxial_variant_survives_a_save_and_open_bit_for_bit() {
    let choice = uniaxial_choice();
    let design = design_with(&choice);
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("must save");
    assert!(text.contains("color_variant"));
    let loaded = round_trip(&design);
    assert_eq!(loaded, design.material);
    assert_eq!(loaded.color_variant_override, Some(choice));
}

#[test]
fn a_biaxial_variant_survives_a_save_and_open_bit_for_bit() {
    let choice = biaxial_choice();
    let design = design_with(&choice);
    let loaded = round_trip(&design);
    assert_eq!(loaded, design.material);
    let restored = loaded.color_variant_override.expect("the variant is kept");
    assert_eq!(restored.absorption, choice.absorption);
    assert!(restored.absorption.beta_ray.is_some());
    assert_eq!(restored.unit, AbsorptionUnit::ModelUnit);
    assert_eq!(restored.reference_path_mm.to_bits(), 7.0f32.to_bits());
}

#[test]
fn a_saved_file_is_stable_through_a_second_round_trip() {
    let design = design_with(&uniaxial_choice());
    let first = design_to_string(&design, None, &DesignExtras::default()).expect("first save");
    let loaded = design_from_str(&first).expect("load").design;
    let second = design_to_string(&loaded, None, &DesignExtras::default()).expect("second save");
    assert_eq!(first, second);
}

#[test]
fn a_variant_file_also_carries_the_legacy_fallback_triple() {
    let design = design_with(&uniaxial_choice());
    let file = design_to_file(&design, None, &DesignExtras::default());
    assert!(file.material.color_variant.is_some());
    let triple = file
        .material
        .body_color_override
        .expect("an older build needs a triple to show");
    assert!(triple.iter().all(|v| v.is_finite() && *v >= 0.0));
    // The same triple is recorded inside the table, to tell an older build's later edit.
    assert_eq!(
        file.material
            .color_variant
            .as_ref()
            .and_then(|table| table.fallback_rgb),
        Some(triple)
    );
    // The N-band form is not written next to a variant: the triple is the whole fallback.
    assert_eq!(file.material.body_color_bands_override, None);
    // Reading ignores the fallback: the selection holds the variant and no legacy color.
    let loaded = design_from_file(file).expect("load").design.material;
    assert!(loaded.color_variant_override.is_some());
    assert_eq!(loaded.body_color_override, None);
    assert_eq!(loaded.body_color_bands_override, None);
}

#[test]
fn a_file_without_the_table_loads_without_a_variant() {
    let mut design = simple_design();
    design.material.name = Some("Sapphire".to_owned());
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("must save");
    assert!(!text.contains("color_variant"));
    let loaded = design_from_str(&text).expect("load").design.material;
    assert_eq!(loaded.color_variant_override, None);
    assert_eq!(loaded, design.material);
}

#[test]
fn a_legacy_triple_file_loads_exactly_as_before() {
    let mut design = simple_design();
    design.material = design.material.with_body_color(Some([0.2, 0.4, 2.8]));
    let loaded = round_trip(&design);
    assert_eq!(loaded.color_variant_override, None);
    assert_eq!(loaded.body_color_override, Some([0.2, 0.4, 2.8]));
    assert_eq!(loaded, design.material);
}

/// The written file with `edit` applied to its variant table, opened again.
fn open_with_edited_variant(
    edit: impl FnOnce(&mut pleochroma_formats::native::ColorVariantTable),
) -> Result<crate::native::LoadedDesign, DesignLoadError> {
    let mut file = design_to_file(
        &design_with(&uniaxial_choice()),
        None,
        &DesignExtras::default(),
    );
    edit(
        file.material
            .color_variant
            .as_mut()
            .expect("a variant table"),
    );
    design_from_file(file)
}

/// The selection of a file whose variant table names something a newer build introduced: the
/// variant is skipped and the legacy triple written next to it is the color.
fn assert_opens_with_the_legacy_triple(
    edit: impl FnOnce(&mut pleochroma_formats::native::ColorVariantTable),
) {
    let triple = design_to_file(
        &design_with(&uniaxial_choice()),
        None,
        &DesignExtras::default(),
    )
    .material
    .body_color_override
    .expect("a fallback triple");
    let loaded = open_with_edited_variant(edit)
        .expect("a name this build does not know must not refuse the design")
        .design
        .material;
    assert_eq!(loaded.color_variant_override, None);
    assert_eq!(
        loaded.body_color_override.map(|rgb| rgb.map(f32::to_bits)),
        Some(triple.map(|v| convert::narrow(v).to_bits()))
    );
}

#[test]
fn an_unknown_band_shape_falls_back_to_the_legacy_triple() {
    assert_opens_with_the_legacy_triple(|table| table.e_ray[1].shape = "gauss".to_owned());
}

#[test]
fn an_unknown_unit_falls_back_to_the_legacy_triple() {
    assert_opens_with_the_legacy_triple(|table| table.unit = "per_inch".to_owned());
}

#[test]
fn a_ray_with_more_bands_than_the_gpu_holds_is_refused_with_a_sentence() {
    let Err(error) = open_with_edited_variant(|table| {
        let band = table.o_ray[0].clone();
        table.o_ray = vec![band; MAX_VARIANT_BANDS + 1];
    }) else {
        panic!("nine bands must not load");
    };
    let message = error.to_string();
    assert!(message.contains("material.color_variant"), "{message}");
    assert!(message.contains("o_ray has 9 bands"), "{message}");
    assert!(message.contains("at most 8"), "{message}");
    // The limit itself loads.
    assert!(
        open_with_edited_variant(|table| {
            let band = table.o_ray[0].clone();
            table.o_ray = vec![band; MAX_VARIANT_BANDS];
        })
        .is_ok()
    );
}

#[test]
fn the_fallback_triple_is_the_face_up_color_of_a_stone_as_wide_as_the_reference() {
    let choice = uniaxial_choice();
    let host = GemMaterial::built_in()
        .iter()
        .find(|m| m.name == "Sapphire")
        .expect("a built-in");
    // The reference path is a stone width (5.5 mm): the color is the tensor over the face-up
    // path of that stone through the host's default frame, not over 5.5 mm of light path.
    let lab = face_up_swatch(
        &choice.absorption,
        choice.unit,
        f64::from(choice.reference_path_mm),
        host,
    )
    .lab;
    let expected = nearest_legacy_rgb(lab);
    let file = design_to_file(&design_with(&choice), None, &DesignExtras::default());
    let written = file
        .material
        .body_color_override
        .expect("a fallback triple");
    assert_eq!(
        written.map(|v| convert::narrow(v).to_bits()),
        expected.map(f32::to_bits)
    );
    // A host that is not a built-in has no frame: the unpolarised color is read instead.
    let mut nameless = design_with(&choice);
    nameless.material.name = Some("Unknown Custom Stone".to_owned());
    let unpolarised = nearest_legacy_rgb(
        body_colors(
            &choice.absorption,
            face_up_swatch_path(choice.unit, choice.reference_path_mm),
            Illuminant::D65,
        )
        .unpolarised
        .lab,
    );
    let file = design_to_file(&nameless, None, &DesignExtras::default());
    assert_eq!(
        file.material
            .body_color_override
            .expect("a fallback triple")
            .map(|v| convert::narrow(v).to_bits()),
        unpolarised.map(f32::to_bits)
    );
}

#[test]
fn an_older_builds_color_edit_beats_the_variant_it_could_not_see() {
    let choice = uniaxial_choice();
    // An older build kept the table, wrote its own triple: the variant is dropped, the cutter's
    // color wins.
    let mut file = design_to_file(&design_with(&choice), None, &DesignExtras::default());
    file.material.body_color_override = Some([0.25, 0.5, 1.75]);
    let loaded = design_from_file(file).expect("load").design.material;
    assert_eq!(loaded.color_variant_override, None);
    assert_eq!(loaded.body_color_override, Some([0.25, 0.5, 1.75]));

    // It cleared the color ("Material Default" there): the variant is dropped too.
    let mut file = design_to_file(&design_with(&choice), None, &DesignExtras::default());
    file.material.body_color_override = None;
    let loaded = design_from_file(file).expect("load").design.material;
    assert_eq!(loaded.color_variant_override, None);
    assert_eq!(loaded.body_color_override, None);

    // An unedited file keeps the variant, and so does a file from before the field existed.
    let file = design_to_file(&design_with(&choice), None, &DesignExtras::default());
    let loaded = design_from_file(file).expect("load").design.material;
    assert_eq!(loaded.color_variant_override, Some(choice.clone()));
    let mut file = design_to_file(&design_with(&choice), None, &DesignExtras::default());
    file.material
        .color_variant
        .as_mut()
        .expect("a variant table")
        .fallback_rgb = None;
    file.material.body_color_override = Some([0.25, 0.5, 1.75]);
    let loaded = design_from_file(file).expect("load").design.material;
    assert_eq!(loaded.color_variant_override, Some(choice));
}

#[test]
fn a_non_finite_or_non_positive_band_value_is_refused() {
    for edit in [
        (|table: &mut pleochroma_formats::native::ColorVariantTable| {
            table.o_ray[0].peak = f64::NAN;
        }) as fn(&mut pleochroma_formats::native::ColorVariantTable),
        |table| table.o_ray[0].width_nm = 0.0,
        |table| table.o_ray[0].peak = -0.1,
        |table| table.reference_path_mm = 0.0,
    ] {
        assert!(open_with_edited_variant(edit).is_err());
    }
}
