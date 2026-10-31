//! Tests of the Stone, Light and Checks logic. Everything runs on a mock platform and
//! the shared editor session; nothing needs a window or a GPU.

use std::sync::Arc;

use pleochroma::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{
            AbsorptionUnit, GemMaterial,
            body_color::BODY_COLOR_PRESETS,
            variants::{ColorVariant, VariantProvenance, built_in_variants},
        },
        raytracer::LightingPreset,
    },
    render_setup::inclusion_scale,
};
use pleochroma_cut_core::{
    material::{
        CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice,
        pattern::{ColorPattern, PatternAxis, TemplateId, template},
    },
    native::{DesignExtras, design_from_str, design_to_string},
    templates::TEMPLATES,
};
use pleochroma_editor::EditorSession;
use pleochroma_web_core::settings::{EXPOSURE_RANGE, HEAD_SHADOW_RANGE};

use super::{
    checks::test_support::{compute_view, first_fix_request},
    *,
};
use crate::{
    app_state::AppState,
    platform::Platform,
    platform_mock::MockPlatform,
    settings::{Backdrop, DEFAULT_LIGHTING_RIG},
};

/// A fresh app state on a mock platform.
fn state() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    AppState::new(platform)
}

/// A session over template `index` (0 is the standard round brilliant) in its own material.
fn template_session(index: usize) -> EditorSession {
    let template = &TEMPLATES[index];
    EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        i32::try_from(index + 1).expect("a small template index"),
    )
}

/// Undoes everything the session can undo.
fn undo_all(state: &mut AppState) {
    while state.session.undo().expect("undo replays").is_some() {}
}

// --- materials ---------------------------------------------------------------------------

#[test]
fn material_rows_list_diamond_with_its_optics() {
    let state = state();
    let rows = material_rows(&state, "");
    let diamond = rows
        .iter()
        .find(|row| row.name == "Diamond")
        .expect("Diamond is built in");
    let ri: f32 = diamond.ri_text.parse().expect("the index is a number");
    assert!((2.40..2.43).contains(&ri), "Diamond's index reads {ri}");
    assert_eq!(diamond.sg_text, "3.52");
    assert!(!diamond.is_custom);
    let detail = diamond.detail_text();
    assert!(detail.starts_with("RI "), "{detail}");
    assert!(detail.ends_with("SG 3.52"), "{detail}");

    let names: Vec<String> = rows
        .iter()
        .map(|row| row.name.to_ascii_lowercase())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "built-ins are listed alphabetically");
}

#[test]
fn material_search_ignores_case_and_padding() {
    let state = state();
    let rows = material_rows(&state, "  SAPP ");
    assert_ne!(rows.len(), 0);
    assert!(
        rows.iter()
            .all(|row| row.name.to_lowercase().contains("sapp"))
    );
    assert_eq!(material_rows(&state, "zzzz-no-such-gem").len(), 0);
}

#[test]
fn picking_a_material_is_one_undoable_edit() {
    let mut state = state();
    state.session = template_session(0);
    assert_eq!(
        state.session.design.material.name.as_deref(),
        Some("Diamond")
    );

    assert_eq!(select_material(&mut state, " sapphire "), Ok(true));
    assert_eq!(
        state.session.design.material.name.as_deref(),
        Some("Sapphire")
    );
    assert_eq!(state.settings.material_name, "Sapphire");
    let rows = material_rows(&state, "sapphire");
    assert!(rows.iter().any(|row| row.is_selected));

    assert_eq!(select_material(&mut state, "Sapphire"), Ok(false));
    assert!(state.session.undo().expect("undo replays").is_some());
    assert_eq!(
        state.session.design.material.name.as_deref(),
        Some("Diamond")
    );
}

#[test]
fn an_unknown_material_is_refused() {
    let mut state = state();
    let error = select_material(&mut state, "Unobtainium").expect_err("not in the catalogue");
    assert!(error.contains("Unobtainium"), "{error}");
}

#[test]
fn the_refractive_index_override_is_validated_and_clearable() {
    let mut state = state();
    state.session = template_session(0);
    assert!(set_refractive_index_override(&mut state, "1.0").is_err());
    assert!(set_refractive_index_override(&mut state, "abc").is_err());
    assert_eq!(set_refractive_index_override(&mut state, "1.76"), Ok(true));
    assert_eq!(
        state.session.design.material.refractive_index_override,
        Some(1.76)
    );
    assert_eq!(set_refractive_index_override(&mut state, "1.76"), Ok(false));
    assert_eq!(set_refractive_index_override(&mut state, " "), Ok(true));
    assert_eq!(
        state.session.design.material.refractive_index_override,
        None
    );
}

#[test]
fn the_custom_material_form_keeps_its_ten_generic_colors() {
    let names = body_color_names();
    assert_eq!(names.len(), 10);
    assert_eq!(names[0], "Material Default");
}

/// The picker position of the last row called `label` (a generic preset follows the variants of
/// the same name, so the last one is the preset).
fn color_row(state: &AppState, label: &str) -> i32 {
    let position = color_rows(state)
        .iter()
        .rposition(|row| row.label == label)
        .expect("the picker lists that color");
    i32::try_from(position).expect("a handful of rows")
}

/// The triple of the generic preset called `label`.
fn preset_rgb(label: &str) -> [f32; 3] {
    BODY_COLOR_PRESETS
        .iter()
        .find(|preset| preset.label == label)
        .expect("a generic preset of that name")
        .absorption_rgb
}

#[test]
fn the_body_color_is_an_undoable_choice() {
    let mut state = state();
    state.session = template_session(0);
    let rows = color_rows(&state);
    assert_eq!(rows[0].label, "Material Default");
    assert!(rows[0].is_selected);
    assert_eq!(rows.iter().filter(|row| row.is_selected).count(), 1);
    assert!(
        rows.iter().all(|row| row.label != CUSTOM_VARIANT_LABEL),
        "a design with no color of its own has no Custom Color row to keep"
    );

    let red = color_row(&state, "Red");
    assert_eq!(set_color_entry(&mut state, red), Ok(true));
    let material = &state.session.design.material;
    assert_eq!(material.body_color_override, Some(preset_rgb("Red")));
    assert_eq!(material.color_variant_override, None);
    assert!(
        color_rows(&state)
            .iter()
            .any(|row| row.label == "Red" && row.is_selected)
    );
    assert_eq!(stone_view(&state).color_label, "Red");

    assert!(set_color_entry(&mut state, 1000).is_err());
    assert!(set_color_entry(&mut state, -1).is_err());
    assert_eq!(set_color_entry(&mut state, 0), Ok(true));
    assert!(state.session.design.material.body_color_override.is_none());

    assert!(state.session.undo().expect("undo replays").is_some());
    assert_eq!(
        state.session.design.material.body_color_override,
        Some(preset_rgb("Red"))
    );
}

/// A Sapphire design (a pleochroic host) on the phone.
fn sapphire_state() -> AppState {
    let mut state = state();
    state.session = template_session(0);
    assert_eq!(select_material(&mut state, "Sapphire"), Ok(true));
    state
}

/// The built-in variants of Sapphire the picker lists: the data file's entries the host accepts.
fn sapphire_variants() -> Vec<ColorVariant> {
    let host = GemMaterial::sapphire();
    built_in_variants("Sapphire")
        .into_iter()
        .filter(|variant| !matches!(variant.provenance, VariantProvenance::MaterialDefault))
        .filter(|variant| variant.validate_for(&host).is_ok())
        .collect()
}

/// A color variant as another device would have saved it into the design file: not one of
/// Sapphire's built-in keys, with a per-axis (uniaxial) tensor the host accepts.
fn carried_choice() -> VariantChoice {
    VariantChoice {
        key: "user.0001".to_owned(),
        label: "Test Pink".to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(550.0, 40.0, 0.35)],
            vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
    }
}

#[test]
fn a_pleochroic_material_lists_its_variants_and_no_generic_colors() {
    let state = sapphire_state();
    let variants = sapphire_variants();
    assert!(!variants.is_empty(), "Sapphire ships color variants");
    let labels: Vec<String> = color_rows(&state)
        .into_iter()
        .map(|row| row.label)
        .collect();
    let mut expected = vec!["Material Default".to_owned()];
    expected.extend(variants.iter().map(|variant| variant.label.clone()));
    assert_eq!(
        labels, expected,
        "the material's own color and its variants: no generic one-color preset (it would drop \
         the pleochroism) and no Custom Color row"
    );
}

#[test]
fn picking_a_variant_copies_it_into_the_design_and_keeps_the_hosts_pleochroism() {
    let mut state = sapphire_state();
    let variant = sapphire_variants()
        .into_iter()
        .next()
        .expect("Sapphire ships color variants");
    let row = color_row(&state, &variant.label);

    assert_eq!(set_color_entry(&mut state, row), Ok(true));
    let material = state.session.design.material.clone();
    assert_eq!(
        material.color_variant_override,
        Some(VariantChoice::from_variant(&variant))
    );
    assert_eq!(material.body_color_override, None);
    assert_eq!(material.body_color_bands_override, None);
    // The resolved stone: the variant's tensor on the host, every other property the host's own.
    let resolved = material.apply_overrides(GemMaterial::sapphire());
    assert_eq!(resolved.absorption, variant.absorption);
    assert_eq!(resolved.absorption_unit, variant.unit);
    assert_eq!(resolved.dispersion, GemMaterial::sapphire().dispersion);
    // The picker and the Stone tab show it, and picking it again changes nothing.
    assert!(
        color_rows(&state)
            .iter()
            .any(|shown| shown.label == variant.label && shown.is_selected)
    );
    assert_eq!(stone_view(&state).color_label, variant.label);
    assert_eq!(set_color_entry(&mut state, row), Ok(false));

    // The design file carries the variant, so a phone opens a desktop design as it was saved.
    let text = design_to_string(&state.session.design, None, &DesignExtras::default())
        .expect("the design encodes");
    let loaded = design_from_str(&text).expect("the design reads back");
    assert_eq!(
        loaded.design.material.color_variant_override,
        state.session.design.material.color_variant_override
    );

    // `Material Default` clears it, and Undo brings it back.
    assert_eq!(set_color_entry(&mut state, 0), Ok(true));
    assert_eq!(state.session.design.material.color_variant_override, None);
    assert!(state.session.undo().expect("undo replays").is_some());
    assert_eq!(state.session.design.material, material);

    // A variant belongs to its material: another material drops it.
    assert_eq!(select_material(&mut state, "Diamond"), Ok(true));
    assert_eq!(state.session.design.material.color_variant_override, None);
}

#[test]
fn a_variant_the_design_file_carries_is_listed_by_its_own_name() {
    let mut state = sapphire_state();
    let carried = carried_choice();
    state.session.design.material = state
        .session
        .design
        .material
        .clone()
        .with_color_variant(Some(carried.clone()));

    let rows = color_rows(&state);
    let mut expected = vec!["Material Default".to_owned()];
    expected.extend(
        sapphire_variants()
            .iter()
            .map(|variant| variant.label.clone()),
    );
    expected.push(carried.label.clone());
    assert_eq!(
        rows.iter().map(|row| row.label.clone()).collect::<Vec<_>>(),
        expected,
        "the built-in variants, then the one the file carries; not shown as a Custom Color"
    );
    assert!(
        rows.iter()
            .any(|row| row.label == carried.label && row.is_selected)
    );
    assert_eq!(stone_view(&state).color_label, carried.label);
    let own = color_row(&state, &carried.label);
    assert_eq!(set_color_entry(&mut state, own), Ok(false));
    assert_eq!(
        state.session.design.material.color_variant_override,
        Some(carried.clone())
    );

    // Picking a built-in variant replaces it, and it is no longer listed.
    let builtin = sapphire_variants()
        .into_iter()
        .next()
        .expect("Sapphire ships color variants");
    let row = color_row(&state, &builtin.label);
    assert_eq!(set_color_entry(&mut state, row), Ok(true));
    assert!(
        color_rows(&state)
            .iter()
            .all(|shown| shown.label != carried.label)
    );
}

#[test]
fn a_custom_color_stays_as_saved_and_is_listed_as_custom_color() {
    let mut state = sapphire_state();
    let designs = [
        // The L*C*h editor's N-band color.
        state.session.design.material.clone().with_body_color_bands(
            Some([0.2, 0.4, 2.8]),
            Some(vec![[520.0, 40.0, 0.25]]),
            Some(1.0),
        ),
        // A per-axis color made in the Custom editor.
        state
            .session
            .design
            .material
            .clone()
            .with_color_variant(Some(VariantChoice {
                key: CUSTOM_VARIANT_KEY.to_owned(),
                label: CUSTOM_VARIANT_LABEL.to_owned(),
                ..carried_choice()
            })),
    ];
    for material in designs {
        state.session.design.material = material.clone();
        let rows = color_rows(&state);
        let last = rows.last().expect("the picker has rows");
        assert_eq!(last.label, "Custom Color");
        assert!(last.is_selected);
        assert!(last.detail.contains("desktop"), "{}", last.detail);
        assert_eq!(rows.iter().filter(|row| row.is_selected).count(), 1);
        assert_eq!(stone_view(&state).color_label, "Custom Color");

        // There is no editor: picking the row keeps the color exactly as it is.
        let custom = i32::try_from(rows.len() - 1).expect("a handful of rows");
        assert_eq!(set_color_entry(&mut state, custom), Ok(false));
        assert_eq!(state.session.design.material, material);
    }

    // A named color replaces it, and the row goes with it.
    let variant = sapphire_variants()
        .into_iter()
        .next()
        .expect("Sapphire ships color variants");
    let row = color_row(&state, &variant.label);
    assert_eq!(set_color_entry(&mut state, row), Ok(true));
    assert!(
        color_rows(&state)
            .iter()
            .all(|shown| shown.label != "Custom Color")
    );
}

#[test]
fn a_single_color_on_a_pleochroic_material_reads_as_legacy() {
    let mut state = sapphire_state();
    state.session.design.material.body_color_override = Some(preset_rgb("Blue"));
    let legacy = "Legacy: Blue (No Pleochroism)";
    assert_eq!(stone_view(&state).color_label, legacy);
    let rows = color_rows(&state);
    assert!(
        rows.iter()
            .any(|row| row.label == legacy && row.is_selected),
        "the picker shows the saved color as it is"
    );
    assert!(rows.iter().all(|row| row.label != "Custom Color"));
}

#[test]
fn a_variant_the_host_does_not_accept_reads_as_custom_color_in_the_tab_and_the_picker() {
    let mut state = state();
    state.session = template_session(0);
    let variant = sapphire_variants()
        .into_iter()
        .find(|variant| variant.absorption.is_pleochroic)
        .expect("Sapphire ships a pleochroic variant");
    // Diamond is a one-color host: a file that carries a Sapphire variant cannot list it.
    state.session.design.material = state
        .session
        .design
        .material
        .clone()
        .with_color_variant(Some(VariantChoice::from_variant(&variant)));
    assert_eq!(stone_view(&state).color_label, CUSTOM_VARIANT_LABEL);
    let rows = color_rows(&state);
    let shown: Vec<&ColorRow> = rows.iter().filter(|row| row.is_selected).collect();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        shown[0].label, CUSTOM_VARIANT_LABEL,
        "the picker and the tab name the same row"
    );
}

#[test]
fn a_design_with_no_material_name_gets_the_plain_list_and_no_variant() {
    let mut state = state();
    state.session = template_session(0);
    state.session.design.material.name = None;
    let rows = color_rows(&state);
    assert_eq!(
        rows.len(),
        1 + BODY_COLOR_PRESETS.len(),
        "Material Default and the nine presets: no variant of a guessed material"
    );
    assert_eq!(rows[0].label, "Material Default");
    let red = color_row(&state, "Red");
    assert_eq!(set_color_entry(&mut state, red), Ok(true));
    assert_eq!(
        state.session.design.material.body_color_override,
        Some(preset_rgb("Red"))
    );
    assert_eq!(stone_view(&state).color_label, "Red");

    // A variant the file carries on such a design is not listed under its name either.
    state.session.design.material = state
        .session
        .design
        .material
        .clone()
        .with_color_variant(Some(carried_choice()));
    assert_eq!(stone_view(&state).color_label, CUSTOM_VARIANT_LABEL);
    assert!(
        color_rows(&state)
            .iter()
            .all(|row| row.label != carried_choice().label)
    );
}

/// A solved two-zone pattern, to stand for the stone's own `color_pattern_override`.
fn solved_pattern() -> ColorPattern {
    let mut pattern = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
    for at in pattern.unsolved_colors() {
        pattern.set_color_bands(at, vec![[520.0, 40.0, 0.25]]);
    }
    pattern
}

#[test]
fn a_body_color_replaces_a_pattern() {
    let mut state = state();
    state.session = template_session(0);
    state.session.design.material.color_pattern_override = Some(solved_pattern());

    let blue = color_row(&state, "Blue");
    assert_eq!(set_color_entry(&mut state, blue), Ok(true));
    let material = &state.session.design.material;
    assert_eq!(material.color_pattern_override, None);
    assert_eq!(material.body_color_override, Some(preset_rgb("Blue")));

    // `Material Default` clears the pattern too, even when no triple is set.
    state.session.design.material.color_pattern_override = Some(solved_pattern());
    state.session.design.material.body_color_override = None;
    assert_eq!(set_color_entry(&mut state, 0), Ok(true));
    assert_eq!(state.session.design.material.color_pattern_override, None);
}

#[test]
fn a_variant_replaces_a_pattern() {
    let mut state = sapphire_state();
    state.session.design.material.color_pattern_override = Some(solved_pattern());
    let variant = sapphire_variants()
        .into_iter()
        .next()
        .expect("Sapphire ships color variants");

    let row = color_row(&state, &variant.label);
    assert_eq!(set_color_entry(&mut state, row), Ok(true));
    let material = &state.session.design.material;
    assert_eq!(material.color_pattern_override, None);
    assert_eq!(
        material.color_variant_override,
        Some(VariantChoice::from_variant(&variant))
    );
}

#[test]
fn a_patterned_stone_on_an_unzoned_material_is_recolorable() {
    let mut state = state();
    state.session = template_session(0);
    assert!(body_color_available(&state));
    state.session.design.material.color_pattern_override = Some(solved_pattern());
    assert!(body_color_available(&state));
}

#[test]
fn a_zoned_material_without_a_pattern_is_not() {
    let mut state = state();
    state.session = template_session(0);
    assert_eq!(select_material(&mut state, "Ametrine"), Ok(true));
    assert_eq!(state.session.design.material.color_pattern_override, None);
    assert!(!body_color_available(&state));
    let error = set_color_entry(&mut state, 0).expect_err("the zones own the color");
    assert!(error.contains("zones"), "{error}");

    // A pattern on the stone is what a pick replaces, so the picker opens up. A zoned
    // material ignores a plain color, so `Material Default`, which restores the zones, is the
    // one choice it lists.
    state.session.design.material.color_pattern_override = Some(solved_pattern());
    assert!(body_color_available(&state));
    let rows = color_rows(&state);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "Material Default");
    assert_eq!(set_color_entry(&mut state, 0), Ok(true));
    assert_eq!(state.session.design.material.color_pattern_override, None);
}

// --- custom materials --------------------------------------------------------------------

#[test]
fn a_custom_material_with_a_refractive_index_of_one_is_refused() {
    let form = CustomMaterialInput {
        name: "Glass X".to_owned(),
        ri: "1.0".to_owned(),
        ..CustomMaterialInput::default()
    };
    let error = custom_material_from_form(&form).expect_err("an index of 1.0 is not a gem");
    assert!(error.contains("between"), "{error}");
}

#[test]
fn a_custom_material_is_added_and_becomes_the_designs_material() {
    let mut state = state();
    state.session = template_session(0);
    let form = CustomMaterialInput {
        name: " Glass X ".to_owned(),
        ri: "1.62".to_owned(),
        specific_gravity: "2.5".to_owned(),
        ..CustomMaterialInput::default()
    };
    assert_eq!(add_custom_material(&mut state, &form), Ok(true));
    assert!(state.props.find_custom("glass x").is_some());
    assert_eq!(
        state.session.design.material.name.as_deref(),
        Some("Glass X")
    );
    assert_eq!(state.settings.material_name, "Glass X");

    let rows = material_rows(&state, "glass x");
    let row = rows
        .iter()
        .find(|row| row.is_custom)
        .expect("the custom material is listed");
    assert!(row.is_selected);
    assert_eq!(row.sg_text, "2.50");
    assert_eq!(
        state.props.custom_specific_gravity(),
        vec![("Glass X".to_owned(), 2.5)]
    );
    let index = state
        .session
        .design
        .effective_refractive_index_with(&state.props.custom_materials);
    assert!((index - 1.62).abs() < 0.01, "the new index shows ({index})");

    undo_all(&mut state);
    assert_eq!(
        state.session.design.material.name.as_deref(),
        Some("Diamond")
    );
}

#[test]
fn a_custom_material_cannot_take_a_built_in_name() {
    let mut state = state();
    let form = CustomMaterialInput {
        name: "diamond".to_owned(),
        ri: "1.62".to_owned(),
        ..CustomMaterialInput::default()
    };
    let error = add_custom_material(&mut state, &form).expect_err("shadows a built-in");
    assert!(error.contains("built-in"), "{error}");
    assert_eq!(state.props.custom_materials.len(), 0);

    let shadow = GemMaterial::new_custom("Diamond", 1.5, 0.0, 0.0, [0.0; 3]);
    assert!(state.props.register_custom(shadow, None).is_err());
}

#[test]
fn registering_a_custom_material_twice_replaces_it() {
    let mut props = PropsState::default();
    props
        .register_custom(
            GemMaterial::new_custom("Zorbium", 1.5, 0.0, 0.0, [0.0; 3]),
            None,
        )
        .expect("registers");
    props
        .register_custom(
            GemMaterial::new_custom("ZORBIUM", 1.7, 0.0, 0.0, [0.0; 3]),
            None,
        )
        .expect("replaces");
    assert_eq!(props.custom_materials.len(), 1);
    assert_eq!(props.custom_materials[0].name, "ZORBIUM");
}

// --- stone -------------------------------------------------------------------------------

#[test]
fn the_stone_width_sets_clears_and_validates() {
    let mut state = state();
    state.session = template_session(0);
    assert_eq!(stone_view(&state).width_text, "");

    assert_eq!(set_stone_width(&mut state, "6.5+0.25"), Ok(true));
    assert_eq!(state.session.design.girdle_diameter_mm, Some(6.75));
    assert_eq!(stone_view(&state).width_text, "6.75");
    assert_eq!(set_stone_width(&mut state, "6.75"), Ok(false));
    assert!(set_stone_width(&mut state, "0").is_err());
    assert!(set_stone_width(&mut state, "abc").is_err());
    assert!(set_stone_width(&mut state, "5000").is_err());

    assert_eq!(set_stone_width(&mut state, ""), Ok(true));
    assert_eq!(state.session.design.girdle_diameter_mm, None);
    assert!(state.session.undo().expect("undo replays").is_some());
    assert_eq!(state.session.design.girdle_diameter_mm, Some(6.75));
}

#[test]
fn the_stone_effects_are_undoable_design_edits() {
    let mut state = state();
    state.session = template_session(0);
    assert!(state.session.design.render_effects.is_default());

    assert_eq!(set_inclusion_haze(&mut state, 0.5), Ok(true));
    let sigma_s = state.session.design.render_effects.inclusion_sigma_s;
    assert!(
        (sigma_s - 0.5).abs() < 1e-9,
        "2 * 0.5^2 is 0.5, got {sigma_s}"
    );
    let position = stone_view(&state).haze_position;
    assert!((position - 0.5).abs() < 1e-5, "{position}");
    assert!((inclusion_scale::position_to_sigma_s(position) - 0.5).abs() < 1e-5);

    assert_eq!(set_frosted_girdle(&mut state, true), Ok(true));
    assert_eq!(set_frosted_girdle(&mut state, true), Ok(false));
    assert!(stone_view(&state).frosted);

    assert_eq!(set_edge_rounding(&mut state, 1.0), Ok(true));
    let radius = state.session.design.render_effects.edge_rounding_radius;
    assert!(
        (radius - 0.03).abs() < 1e-9,
        "clamped to the ceiling, got {radius}"
    );

    assert_eq!(set_crystal_axis(&mut state, true, 100.0, 400.0), Ok(true));
    let effects = state.session.design.render_effects;
    assert!(effects.c_axis_override_enabled);
    assert_eq!(effects.c_axis_tilt_deg, 90.0);
    assert_eq!(effects.c_axis_azimuth_deg, 360.0);

    undo_all(&mut state);
    assert!(state.session.design.render_effects.is_default());
}

#[test]
fn the_stone_view_describes_the_material() {
    let mut state = state();
    let view = stone_view(&state);
    assert_eq!(view.material_title, "Not Set");

    state.session = template_session(0);
    let view = stone_view(&state);
    assert_eq!(view.material_title, "Diamond");
    assert!(
        view.material_detail.contains("SG 3.52"),
        "{}",
        view.material_detail
    );
    assert!(view.effective_ri_text.starts_with("Effective RI "));
    assert_eq!(view.color_label, "Material Default");
    assert!(view.color_enabled);
    assert!(
        !view.axis_available,
        "diamond is isotropic: no crystal axis"
    );

    assert_eq!(select_material(&mut state, "Quartz"), Ok(true));
    assert!(stone_view(&state).axis_available);
}

// --- lighting ----------------------------------------------------------------------------

#[test]
fn the_lighting_list_leaves_out_the_uv_lamps() {
    let rows = lighting_rows();
    assert_eq!(rows.len(), LightingPreset::ALL.len() - 2);
    assert_eq!(rows.len(), 13);
    assert!(rows.iter().all(|row| !row.name.contains("UV")));
    assert_eq!(
        rows[0].name, DEFAULT_LIGHTING_RIG,
        "the default rig comes first"
    );
    assert!(
        rows.iter()
            .all(|row| !row.title.is_empty() && !row.description.is_empty())
    );
}

#[test]
fn the_lighting_choice_persists_in_the_settings() {
    let mut state = state();
    assert_eq!(state.settings.lighting.preset_name, DEFAULT_LIGHTING_RIG);
    set_lighting(&mut state, "Window daylight").expect("an offered rig");
    assert_eq!(state.settings.lighting.preset_name, "Window daylight");
    assert_eq!(lighting_view(&state).preset_name, "Window daylight");

    assert!(set_lighting(&mut state, "UV lamp 365 nm").is_err());
    assert!(set_lighting(&mut state, "No such rig").is_err());
    assert_eq!(state.settings.lighting.preset_name, "Window daylight");
}

#[test]
fn light_values_are_clamped_and_stored() {
    let mut state = state();
    set_light_values(
        &mut state,
        LightValues {
            exposure: 50.0,
            azimuth_deg: -30.0,
            elevation_deg: 200.0,
            head_shadow_deg: 99.0,
            surface_glare_pct: 47.0,
        },
    );
    let lighting = &state.settings.lighting;
    assert_eq!(lighting.exposure, EXPOSURE_RANGE.1);
    assert_eq!(lighting.azimuth_deg, 330.0);
    assert_eq!(lighting.elevation_deg, ELEVATION_RANGE_DEG.1);
    assert_eq!(lighting.head_shadow_deg, HEAD_SHADOW_RANGE.1);
    assert_eq!(state.settings.lighting.surface_glare_pct, 45.0);
    assert_eq!(lighting_view(&state).values.surface_glare_pct, 45.0);

    set_light_values(
        &mut state,
        LightValues {
            exposure: f32::NAN,
            azimuth_deg: f32::NAN,
            elevation_deg: f32::NAN,
            head_shadow_deg: f32::NAN,
            surface_glare_pct: f32::NAN,
        },
    );
    assert_eq!(state.settings.lighting.exposure, 1.0);
    assert_eq!(
        state.settings.lighting.surface_glare_pct,
        DEFAULT_SURFACE_GLARE_PCT
    );
}

#[test]
fn the_backdrop_and_the_rig_decide_which_sliders_apply() {
    let mut state = state();
    set_backdrop(&mut state, Backdrop::White);
    let view = lighting_view(&state);
    assert_eq!(view.backdrop_index, 2);
    assert_eq!(backdrop_from_index(view.backdrop_index), Backdrop::White);
    assert_eq!(backdrop_from_index(99), Backdrop::White);
    assert_eq!(backdrop_from_index(1), Backdrop::Grey);
    for backdrop in [
        Backdrop::AsLit,
        Backdrop::Grey,
        Backdrop::White,
        Backdrop::Black,
    ] {
        assert_eq!(backdrop_from_index(backdrop_index(backdrop)), backdrop);
    }

    assert!(view.uses_light_direction);
    assert!(view.uses_head_shadow, "the light tent is a lit model");
    set_lighting(&mut state, "Grading tray (D65 hemisphere + head shadow)").expect("offered");
    assert!(!lighting_view(&state).uses_light_direction);
    set_lighting(&mut state, "D65 Daylight (6500K)").expect("offered");
    assert!(
        !lighting_view(&state).uses_head_shadow,
        "a studio rig has no head shadow"
    );
}

#[test]
fn the_render_settings_follow_the_settings_and_the_design() {
    let mut state = state();
    state.session = template_session(0);
    set_stone_width(&mut state, "7.5").expect("a width");
    set_frosted_girdle(&mut state, true).expect("frosted");
    set_lighting(&mut state, "Window daylight").expect("offered");

    let compact = render_settings(&state, true);
    assert_eq!(compact.lighting, "Window daylight");
    assert!(compact.frosted_girdle);
    assert!(compact.link_material);
    assert_eq!(compact.stone_width_mm, 7.5);
    assert_eq!(
        compact.target_spp,
        state.settings.render.live_spp_compact.clamp(
            pleochroma_web_core::settings::LIVE_SPP_RANGE.0,
            pleochroma_web_core::settings::LIVE_SPP_RANGE.1,
        )
    );
}

// --- checks ------------------------------------------------------------------------------

#[test]
fn a_solved_template_has_a_verdict_and_proportions() {
    let session = template_session(0);
    let solved = session.design.solve().expect("the template solves");
    let (view, _) = compute_view(&session.design, &solved);
    assert!(view.ready);
    assert_ne!(view.verdict_text.len(), 0);
    assert!((0..=2).contains(&view.verdict_level));
    let table = view
        .proportions
        .iter()
        .find(|row| row.label == "Table")
        .expect("a table row");
    assert!(table.value.ends_with('%'), "{}", table.value);
    assert!(view.proportions.len() >= 8);
    assert_ne!(view.preform.shape_label.len(), 0);
    assert!(view.preform.editable);
}

#[test]
fn a_design_without_tiers_has_nothing_to_check() {
    let session = EditorSession::fresh();
    let solved = session.design.solve().expect("an empty design solves");
    let (view, _) = compute_view(&session.design, &solved);
    assert_eq!(view.proportions.len(), 0);
    assert_eq!(view.verdict_level, 0);
    assert!(
        view.verdict_text.contains("Nothing to check"),
        "{}",
        view.verdict_text
    );
    assert_eq!(view.warnings.len(), 0);
}

#[test]
fn a_girdle_diameter_gives_a_weight_estimate() {
    let mut state = state();
    state.session = template_session(0);
    set_stone_width(&mut state, "6.5").expect("a width");
    let solved = state.session.design.solve().expect("the template solves");
    let (view, _) = compute_view(&state.session.design, &solved);
    let carat = view
        .carat
        .expect("a weight needs a width and a specific gravity");
    assert!(carat.ends_with("(est.)"), "{carat}");
    assert!(
        view.yield_lines
            .iter()
            .any(|line| line.starts_with("Weight"))
    );
    assert_eq!(view.preform.girdle_mm, "6.5");
    assert!(
        view.preform.half_width_mm.contains("mm"),
        "{}",
        view.preform.half_width_mm
    );
}

#[test]
fn without_a_matching_solve_the_checks_say_so() {
    let mut state = state();
    state.session = template_session(0);
    let view = checks(&state);
    assert!(!view.ready);
    assert_ne!(view.note.len(), 0);
    assert_eq!(view.warnings.len(), 0);
    assert!(
        view.preform.editable,
        "the preform fields work without a solve"
    );
    assert_eq!(
        view.preform.half_width,
        trimmed(state.session.design.preform.half_width, 4)
    );
}

#[test]
fn preform_edits_round_trip_through_undo() {
    let mut state = state();
    state.session = template_session(0);
    let before = state.session.design.preform.clone();

    assert_eq!(
        set_preform_field(&mut state, PreformField::HalfWidth, "1.25"),
        Ok(true)
    );
    assert!((state.session.design.preform.half_width - 1.25).abs() < 1e-12);
    assert_eq!(
        set_preform_field(&mut state, PreformField::HalfWidth, "1.25"),
        Ok(false)
    );
    assert_eq!(
        set_preform_field(&mut state, PreformField::LengthOverWidth, "1.5"),
        Ok(true)
    );
    assert_eq!(
        set_preform_field(&mut state, PreformField::Depth, "2+0.5"),
        Ok(true)
    );
    assert!((state.session.design.preform.depth - 2.5).abs() < 1e-12);

    assert!(set_preform_field(&mut state, PreformField::Depth, "-1").is_err());
    assert!(set_preform_field(&mut state, PreformField::Depth, "").is_err());
    assert!(set_preform_field(&mut state, PreformField::HalfWidth, "abc").is_err());

    undo_all(&mut state);
    assert_eq!(state.session.design.preform, before);
}

#[test]
fn the_girdle_and_gravity_fields_may_be_cleared() {
    let mut state = state();
    state.session = template_session(0);
    assert_eq!(
        set_preform_field(&mut state, PreformField::GirdleMm, "7"),
        Ok(true)
    );
    assert_eq!(state.session.design.girdle_diameter_mm, Some(7.0));
    assert_eq!(
        set_preform_field(&mut state, PreformField::SpecificGravity, "3.5"),
        Ok(true)
    );
    assert_eq!(
        state.session.design.material.specific_gravity_override,
        Some(3.5)
    );
    assert!(set_preform_field(&mut state, PreformField::SpecificGravity, "-2").is_err());
    assert_eq!(
        set_preform_field(&mut state, PreformField::SpecificGravity, ""),
        Ok(true)
    );
    assert_eq!(
        state.session.design.material.specific_gravity_override,
        None
    );
    assert_eq!(
        set_preform_field(&mut state, PreformField::GirdleMm, ""),
        Ok(true)
    );
    assert_eq!(state.session.design.girdle_diameter_mm, None);
}

#[test]
fn preform_fields_have_stable_ids() {
    assert_eq!(
        PreformField::from_id("half-width"),
        Some(PreformField::HalfWidth)
    );
    assert_eq!(
        PreformField::from_id("length-width"),
        Some(PreformField::LengthOverWidth)
    );
    assert_eq!(PreformField::from_id("depth"), Some(PreformField::Depth));
    assert_eq!(
        PreformField::from_id("girdle-mm"),
        Some(PreformField::GirdleMm)
    );
    assert_eq!(
        PreformField::from_id("sg"),
        Some(PreformField::SpecificGravity)
    );
    assert_eq!(PreformField::from_id("nope"), None);
}

/// The first template whose checks offer a fix, with a state holding it.
fn state_with_a_fix() -> Option<(AppState, FixRequest)> {
    (0..TEMPLATES.len()).find_map(|index| {
        let mut state = state();
        state.session = template_session(index);
        let solved = state.session.design.solve().ok()?;
        let request = first_fix_request(&state.session.design, &solved)?;
        Some((state, request))
    })
}

#[test]
fn a_fix_plans_and_applies_as_one_undoable_step() {
    let Some((mut state, request)) = state_with_a_fix() else {
        // No template needs a fix; the planning path is covered by the editor's own tests.
        return;
    };
    let before = state.session.design.tiers.clone();
    let Ok(planned) = request.plan() else {
        // The planner may decline a finding it cannot fix safely.
        return;
    };
    let applied = apply_planned_fix(&mut state, planned).expect("the plan fits its design");
    assert_ne!(applied.message.len(), 0);
    assert_ne!(state.session.design.tiers, before);
    assert!(state.session.undo().expect("undo replays").is_some());
    assert_eq!(state.session.design.tiers, before);
}

#[test]
fn a_plan_for_a_design_that_moved_on_is_refused() {
    let Some((mut state, request)) = state_with_a_fix() else {
        return;
    };
    let Ok(planned) = request.plan() else {
        return;
    };
    state.session.design.tiers.pop();
    let error = apply_planned_fix(&mut state, planned).expect_err("the design changed");
    assert!(error.contains("changed"), "{error}");
}

#[test]
fn a_fix_needs_a_current_solve() {
    let state = state();
    let error = fix_request(&state, 0).expect_err("nothing is solved");
    assert!(error.contains("Solve"), "{error}");
}
