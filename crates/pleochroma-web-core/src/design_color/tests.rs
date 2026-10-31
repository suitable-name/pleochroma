//! The browser's design color: stored records, the keyed combo on a pleochroic and an isotropic
//! stone, and the per-axis Custom editor.

use super::*;
use pleochroma::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::body_color::BODY_COLOR_PRESETS,
};
use pleochroma_editor::lch_color::solve_lch;
use std::sync::atomic::AtomicBool;

/// A design that names `name` and carries no color.
fn design_of(name: &str) -> Design {
    let mut design = pleochroma_editor::EditorSession::fresh().design;
    design.material = MaterialSelection {
        name: Some(name.to_owned()),
        ..MaterialSelection::default()
    };
    design
}

/// A saved uniaxial variant of `material`, one energy band and one wavelength band.
fn saved_uniaxial(material: &str) -> ColorVariant {
    ColorVariant {
        key: "user.00000000-0000-4000-8000-000000000001".to_owned(),
        label: "My Pink".to_owned(),
        material: material.to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(550.0, 1500.5, 0.35)],
            vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 6.0,
        provenance: VariantProvenance::User,
    }
}

/// A saved biaxial variant of `material`.
fn saved_biaxial(material: &str) -> ColorVariant {
    ColorVariant {
        key: "user.00000000-0000-4000-8000-000000000002".to_owned(),
        label: "My Violet".to_owned(),
        material: material.to_owned(),
        absorption: AbsorptionTensor::biaxial(
            vec![AbsorptionBand::new(640.0, 44.4, 0.31)],
            vec![AbsorptionBand::energy(550.0, 1500.5, 0.35)],
            vec![AbsorptionBand::new(460.0, 39.9, 0.55)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
        provenance: VariantProvenance::User,
    }
}

/// Every pending row of `editor` solved, as the page's Worker would.
fn solve_all(editor: &mut AxisEditor) {
    for (row, lch) in editor.pending() {
        let solve =
            solve_lch(lch, editor.path_mm(), &AtomicBool::new(false)).expect("not cancelled");
        assert!(editor.store_solve(row, solve), "a fresh solve is kept");
    }
}

fn bits(tensor: &AbsorptionTensor) -> Vec<u32> {
    let mut sets = vec![&tensor.o_ray, &tensor.e_ray];
    if let Some(beta) = &tensor.beta_ray {
        sets.push(beta);
    }
    sets.into_iter()
        .flatten()
        .flat_map(|band| [band.center_nm, band.width_nm, band.peak].map(f32::to_bits))
        .collect()
}

// -- records ---------------------------------------------------------------------------

#[test]
fn a_uniaxial_and_a_biaxial_record_round_trip_bit_for_bit() {
    for variant in [saved_uniaxial("Sapphire"), saved_biaxial("Tanzanite")] {
        let record = record_for(&variant).expect("a record");
        assert_eq!(record.key, variant.key);
        assert_eq!(record.material_name, variant.material);
        assert_eq!(record.version, RECORD_VERSION);
        let back = variant_from_record(&record).expect("reads back");
        assert_eq!(back, variant);
        assert_eq!(bits(&back.absorption), bits(&variant.absorption));
        assert_eq!(
            back.absorption.beta_ray.is_some(),
            variant.absorption.beta_ray.is_some()
        );
        assert_eq!(
            back.reference_path_mm.to_bits(),
            variant.reference_path_mm.to_bits()
        );
    }
}

#[test]
fn the_record_json_is_the_vault_shape() {
    let record = record_for(&saved_uniaxial("Sapphire")).expect("a record");
    let json: serde_json::Value = serde_json::from_str(&record.variant_json).expect("JSON");
    assert_eq!(json["unit"], "per_mm");
    assert_eq!(json["reference_path_mm"], 6.0);
    assert_eq!(json["absorption"]["pleochroic"], true);
    assert_eq!(json["absorption"]["o_ray"][0]["shape"], "energy");
    assert_eq!(json["absorption"]["e_ray"][0]["shape"], "wavelength");
    // An absent beta set is not written (the vault's rule), and reads back absent.
    assert!(json["absorption"].get("beta_ray").is_none());
    let mut model_unit = saved_uniaxial("Sapphire");
    model_unit.unit = AbsorptionUnit::ModelUnit;
    let record = record_for(&model_unit).expect("a record");
    assert!(record.variant_json.contains("\"model_unit\""));
    assert_eq!(
        variant_from_record(&record).expect("reads").unit,
        AbsorptionUnit::ModelUnit
    );
}

#[test]
fn a_record_the_page_cannot_use_is_refused_with_a_sentence() {
    let good = record_for(&saved_uniaxial("Sapphire")).expect("a record");
    let newer = ColorVariantRecord {
        version: RECORD_VERSION + 1,
        ..good.clone()
    };
    assert!(
        variant_from_record(&newer)
            .expect_err("newer")
            .contains("newer version")
    );
    let damaged = ColorVariantRecord {
        variant_json: "{\"unit\":\"per_mm\"".to_owned(),
        ..good
    };
    assert!(
        variant_from_record(&damaged)
            .expect_err("damaged")
            .contains("damaged")
    );
}

#[test]
fn a_variant_that_cannot_be_stored_is_refused() {
    let mut blank = saved_uniaxial("Sapphire");
    blank.label = "  ".to_owned();
    assert!(record_for(&blank).expect_err("blank").contains("name"));
    let mut foreign = saved_uniaxial("Sapphire");
    foreign.key = "sapphire.pink".to_owned();
    assert!(
        record_for(&foreign)
            .expect_err("built-in key")
            .contains("user.")
    );
    let mut nan = saved_uniaxial("Sapphire");
    nan.absorption.o_ray[0].peak = f32::NAN;
    assert!(record_for(&nan).expect_err("NaN").contains("finite"));
    let mut nameless = saved_uniaxial("Sapphire");
    nameless.material = String::new();
    assert!(record_for(&nameless).is_err());
}

#[test]
fn a_user_key_is_user_dot_a_uuid() {
    let key = user_key([0xAB; 16]);
    assert!(key.starts_with(USER_KEY_PREFIX));
    assert_eq!(key.len(), USER_KEY_PREFIX.len() + 36);
    assert_ne!(user_key([1; 16]), user_key([2; 16]));
}

// -- the combo -------------------------------------------------------------------------

fn presets(colors: &DesignColors) -> usize {
    colors
        .items
        .iter()
        .filter(|item| matches!(item.entry, ColorEntry::Preset(_)))
        .count()
}

fn variant_rows(colors: &DesignColors) -> usize {
    colors
        .items
        .iter()
        .filter(|item| matches!(item.entry, ColorEntry::Variant { .. }))
        .count()
}

#[test]
fn sapphire_lists_its_variants_and_no_generic_presets() {
    let colors = DesignColors::for_design(None, &design_of("Sapphire"), &[], &[]);
    assert_eq!(colors.items[0].entry, ColorEntry::MaterialDefault);
    assert_eq!(
        colors.items.last().map(|i| &i.entry),
        Some(&ColorEntry::Custom)
    );
    assert_eq!(
        presets(&colors),
        0,
        "a pleochroic host never lists the one-color presets"
    );
    assert!(
        variant_rows(&colors) >= 1,
        "the data file's Sapphire variants"
    );
    assert_eq!(colors.selected, 0);
    assert!(!colors.legacy_note);
    assert_eq!(colors.labels().len(), colors.items.len());
    assert_eq!(colors.tips().len(), colors.items.len());
    assert_eq!(colors.swatch_bytes().len(), colors.items.len());
}

#[test]
fn diamond_lists_the_nine_presets_between_its_variants_and_custom() {
    let colors = DesignColors::for_design(None, &design_of("Diamond"), &[], &[]);
    assert_eq!(presets(&colors), BODY_COLOR_PRESETS.len());
    assert_eq!(
        colors.items.last().map(|i| &i.entry),
        Some(&ColorEntry::Custom)
    );
    let first_preset = colors
        .items
        .iter()
        .position(|item| matches!(item.entry, ColorEntry::Preset(_)))
        .expect("a preset row");
    assert!(
        colors.items[..first_preset]
            .iter()
            .all(|item| !matches!(item.entry, ColorEntry::Preset(_)))
    );
}

#[test]
fn a_saved_variant_is_listed_for_its_own_material_only() {
    let saved = [saved_uniaxial("Sapphire")];
    let sapphire = DesignColors::for_design(None, &design_of("Sapphire"), &[], &saved);
    let plain = DesignColors::for_design(None, &design_of("Sapphire"), &[], &[]);
    assert_eq!(sapphire.items.len(), plain.items.len() + 1);
    let at = sapphire
        .items
        .iter()
        .position(|item| item.label == "My Pink")
        .expect("the saved variant");
    assert!(sapphire.items[at].tooltip.contains("Saved by you."));
    // After the built-in variants, before Custom.
    assert_eq!(at, sapphire.items.len() - 2);
    let diamond = DesignColors::for_design(None, &design_of("Diamond"), &[], &saved);
    assert!(diamond.items.iter().all(|item| item.label != "My Pink"));
}

#[test]
fn picking_a_row_stores_a_variant_snapshot_and_the_combo_then_shows_it() {
    let saved = [saved_uniaxial("Sapphire")];
    let design = design_of("Sapphire");
    let colors = DesignColors::for_design(None, &design, &[], &saved);
    let at = colors
        .items
        .iter()
        .position(|item| item.label == "My Pink")
        .expect("the saved variant");
    let index = i32::try_from(at).expect("a small index");
    let picked = colors.material_for_apply(design.material.clone(), index, &design.material);
    let choice = picked.color_variant_override.as_ref().expect("a variant");
    assert_eq!(choice.key, saved[0].key);
    assert!(
        choice.absorption.is_pleochroic,
        "a snapshot keeps the pleochroism"
    );
    let mut next = design;
    next.material = picked;
    let shown = DesignColors::for_design(Some(&colors), &next, &[], &saved);
    assert_eq!(shown.selected, at);
    assert_eq!(shown.items, colors.items, "the cached rows are reused");
    // Material Default clears it again.
    let cleared = shown.material_for_apply(next.material.clone(), 0, &next.material);
    assert_eq!(cleared.color_variant_override, None);
    // Custom keeps it.
    let custom = i32::try_from(shown.items.len() - 1).expect("a small index");
    assert!(shown.is_custom(custom));
    let kept = shown.material_for_apply(next.material.clone(), custom, &next.material);
    assert_eq!(
        kept.color_variant_override,
        next.material.color_variant_override
    );
    // An index outside the list falls back to the fixed-index contract and does not panic.
    let _ = shown.material_for_apply(next.material.clone(), 99, &next.material);
}

#[test]
fn a_legacy_preset_on_sapphire_shows_the_legacy_note_and_diamond_does_not() {
    let mut sapphire = design_of("Sapphire");
    sapphire.material.body_color_override = Some(BODY_COLOR_PRESETS[1].absorption_rgb);
    let colors = DesignColors::for_design(None, &sapphire, &[], &[]);
    assert!(colors.legacy_note);
    assert!(colors.items[colors.selected].label.starts_with("Legacy:"));
    let mut diamond = design_of("Diamond");
    diamond.material.body_color_override = Some(BODY_COLOR_PRESETS[1].absorption_rgb);
    let colors = DesignColors::for_design(None, &diamond, &[], &[]);
    assert!(!colors.legacy_note);
}

// -- the per-axis editor ---------------------------------------------------------------

#[test]
fn the_editor_has_one_row_per_direction_of_the_host() {
    let open = |name: &str| {
        let design = design_of(name);
        AxisEditor::open(&design.material, host_material(&design, &[]), 7.0, 5.0)
    };
    let sapphire = open("Sapphire");
    assert_eq!(sapphire.rank(), TensorRank::Uniaxial);
    assert_eq!(sapphire.labels(), ["Along the Axis", "Across the Axis"]);
    assert_eq!(sapphire.rows().len(), 2);
    let tanzanite = open("Tanzanite");
    assert_eq!(tanzanite.labels(), ["Alpha", "Beta", "Gamma"]);
    assert_eq!(tanzanite.hints().len(), 3);
    let diamond = open("Diamond");
    assert_eq!(diamond.labels(), ["Color"]);
    // Opening on a stone with no color edits nothing until it is touched.
    assert!(diamond.applied_to(&design_of("Diamond").material).is_none());
}

#[test]
fn a_sapphire_custom_color_is_a_pleochroic_tensor_the_host_accepts() {
    let design = design_of("Sapphire");
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host.clone()), 7.0, 5.0);
    assert!(!editor.is_ready());
    assert_eq!(editor.pending().len(), 2);
    assert!(
        editor.choice().is_none(),
        "no stored color before the rows are solved"
    );
    // The sliders move the Across row: the editor is touched, and its answer is for the new target.
    editor.select(1);
    assert_eq!(editor.active(), 1);
    let stale = solve_lch(editor.active_lch(), 5.0, &AtomicBool::new(false)).expect("solves");
    editor.set_active_lch([60.0, 35.0, 120.0]);
    assert!(
        !editor.store_solve(1, stale),
        "an answer for an older target is dropped"
    );
    solve_all(&mut editor);
    assert!(editor.is_ready());
    let preview = editor.preview();
    assert_eq!(preview.swatches.len(), 3, "face-up and one per direction");
    assert_eq!(preview.labels, ["Face-Up", "Along", "Across"]);
    assert_eq!(preview.captions.len(), 2);
    assert!(preview.captions[0].starts_with("Along the Axis: "));
    let applied = editor.applied_to(&design.material).expect("an edit");
    let choice = applied
        .color_variant_override
        .as_ref()
        .expect("stored as a variant");
    assert_eq!(
        choice.key,
        pleochroma_cut_core::material::CUSTOM_VARIANT_KEY
    );
    assert!(choice.absorption.is_pleochroic);
    let traced = applied.apply_overrides(host.clone());
    assert!(
        traced.absorption.is_pleochroic,
        "the stone keeps its pleochroism"
    );
    assert_eq!(traced.name, host.name);
    assert_eq!(traced.dispersion, host.dispersion);
}

#[test]
fn a_diamond_custom_color_stays_isotropic_with_the_old_four_swatch_strip() {
    let design = design_of("Diamond");
    let host = host_material(&design, &[]).expect("Diamond resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    editor.set_active_lch([55.0, 40.0, 260.0]);
    solve_all(&mut editor);
    let preview = editor.preview();
    assert_eq!(preview.swatches.len(), 4);
    assert_eq!(preview.labels.len(), 4);
    let applied = editor.applied_to(&design.material).expect("an edit");
    let choice = applied.color_variant_override.expect("a variant");
    assert!(!choice.absorption.is_pleochroic);
}

#[test]
fn a_biaxial_host_gets_three_rows_and_a_trichroic_tensor() {
    let design = design_of("Tanzanite");
    let host = host_material(&design, &[]).expect("Tanzanite resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host.clone()), 7.0, 5.0);
    assert_eq!(editor.rows().len(), 3);
    editor.set_active_lch([40.0, 50.0, 280.0]);
    solve_all(&mut editor);
    assert_eq!(editor.preview().swatches.len(), 4);
    let choice = editor.choice().expect("ready");
    assert!(choice.absorption.beta_ray.is_some());
    assert!(
        saved_variant_fits(&editor, &host),
        "the stored color validates against the biaxial host"
    );
}

/// Whether the editor's rows can be saved as a variant of `host`.
fn saved_variant_fits(editor: &AxisEditor, host: &GemMaterial) -> bool {
    editor
        .variant_to_save("Deep Violet", user_key([7; 16]))
        .is_ok_and(|variant| variant.validate_for(host).is_ok())
}

#[test]
fn an_untouched_editor_on_a_colored_stone_applies_nothing() {
    let design = design_of("Sapphire");
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    let selection = design.material.with_color_variant(Some(
        pleochroma_cut_core::material::VariantChoice::from_variant(&saved_uniaxial("Sapphire")),
    ));
    let mut editor = AxisEditor::open(&selection, Some(host), 7.0, 5.0);
    solve_all(&mut editor);
    assert!(
        editor.applied_to(&selection).is_none(),
        "the seed is lossy; keep the stored color"
    );
    editor.set_active_lch([30.0, 20.0, 40.0]);
    solve_all(&mut editor);
    assert!(editor.applied_to(&selection).is_some());
}

#[test]
fn save_as_variant_builds_a_user_variant_of_the_hosts_material() {
    let design = design_of("Sapphire");
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    assert!(
        editor
            .variant_to_save("Mine", user_key([1; 16]))
            .expect_err("not solved yet")
            .contains("matching")
    );
    solve_all(&mut editor);
    assert!(
        editor
            .variant_to_save("  ", user_key([1; 16]))
            .expect_err("blank")
            .contains("Name")
    );
    let variant = editor
        .variant_to_save("  Rose Pink ", user_key([1; 16]))
        .expect("saves");
    assert_eq!(variant.label, "Rose Pink");
    assert_eq!(variant.material, "Sapphire");
    assert_eq!(variant.unit, AbsorptionUnit::PerMm);
    assert_eq!(variant.provenance, VariantProvenance::User);
    assert!(variant.absorption.is_pleochroic);
    // It can go straight into the browser's store.
    let record = record_for(&variant).expect("a record");
    assert_eq!(variant_from_record(&record).expect("reads"), variant);
}

#[test]
fn a_design_without_a_resolvable_material_keeps_one_legacy_color() {
    let design = design_of("No Such Material");
    assert!(host_material(&design, &[]).is_none());
    let mut editor = AxisEditor::open(&design.material, None, 7.0, 5.0);
    assert_eq!(editor.rank(), TensorRank::Isotropic);
    assert_eq!(editor.rows(), [DEFAULT_LCH]);
    editor.set_active_lch([50.0, 30.0, 265.0]);
    solve_all(&mut editor);
    let applied = editor.applied_to(&design.material).expect("an edit");
    assert!(applied.color_variant_override.is_none());
    assert!(applied.body_color_bands_override.is_some());
    assert!(
        editor
            .variant_to_save("Blue", user_key([3; 16]))
            .expect_err("no host")
            .contains("one material")
    );
}

// -- convert, badge, save and the list key ----------------------------------------------

#[test]
fn convert_on_a_legacy_color_stores_a_per_axis_custom_color_without_a_slider() {
    let mut design = design_of("Sapphire");
    design.material.body_color_override = Some(BODY_COLOR_PRESETS[1].absorption_rgb);
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    assert!(legacy_color_on_pleochroic_host(&design.material, &host));
    let mut editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    assert_eq!(editor.rows().len(), 2, "the legacy color seeds both rows");
    solve_all(&mut editor);
    let applied = editor
        .applied_to(&design.material)
        .expect("an untouched Convert still converts");
    let choice = applied
        .color_variant_override
        .as_ref()
        .expect("stored as a variant");
    assert_eq!(
        choice.key,
        pleochroma_cut_core::material::CUSTOM_VARIANT_KEY
    );
    assert!(choice.absorption.is_pleochroic, "a per-axis color");
    assert_eq!(
        applied.body_color_override, None,
        "the legacy triple is gone"
    );
}

#[test]
fn an_untouched_apply_on_a_one_color_stone_with_a_preset_changes_nothing() {
    let mut design = design_of("Diamond");
    design.material.body_color_override = Some(BODY_COLOR_PRESETS[1].absorption_rgb);
    let host = host_material(&design, &[]).expect("Diamond resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    solve_all(&mut editor);
    assert!(
        editor.applied_to(&design.material).is_none(),
        "a preset on a one-color stone is not a loss; only a touched editor re-solves it"
    );
    assert!(editor.nothing_to_apply().contains("already"));
}

#[test]
fn an_unsolved_editor_says_to_wait_when_apply_changes_nothing() {
    let design = design_of("Sapphire");
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    let editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    assert!(editor.applied_to(&design.material).is_none());
    assert!(editor.nothing_to_apply().contains("Wait"));
}

#[test]
fn the_badge_says_matching_until_every_row_is_solved() {
    let design = design_of("Tanzanite");
    let host = host_material(&design, &[]).expect("Tanzanite resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    assert_eq!(editor.preview().badge, MATCHING_BADGE);
    let (row, lch) = editor.pending()[0];
    let solve = solve_lch(lch, editor.path_mm(), &AtomicBool::new(false)).expect("solves");
    assert!(editor.store_solve(row, solve));
    assert!(!editor.is_ready(), "two rows are still open");
    assert_eq!(
        editor.preview().badge,
        MATCHING_BADGE,
        "the badge does not go empty between the answers"
    );
    solve_all(&mut editor);
    let badge = editor.preview().badge;
    assert!(!badge.is_empty() && badge != MATCHING_BADGE, "{badge:?}");
}

#[test]
fn a_custom_color_and_a_saved_variant_store_the_stone_width_not_the_solver_path() {
    let design = design_of("Sapphire");
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host), 6.5, 9.75);
    assert_eq!(editor.stone_width_mm().to_bits(), 6.5_f64.to_bits());
    assert_eq!(editor.path_mm().to_bits(), 9.75_f64.to_bits());
    editor.set_active_lch([40.0, 30.0, 250.0]);
    solve_all(&mut editor);
    let choice = editor.choice().expect("ready");
    assert_eq!(choice.reference_path_mm.to_bits(), 6.5_f32.to_bits());
    let variant = editor
        .variant_to_save("Wide", user_key([9; 16]))
        .expect("saves");
    assert_eq!(variant.reference_path_mm.to_bits(), 6.5_f32.to_bits());
}

#[test]
fn the_stone_width_of_a_design_is_its_girdle_or_seven_millimetres() {
    let mut design = design_of("Sapphire");
    assert_eq!(stone_width_mm(&design).to_bits(), 7.0_f64.to_bits());
    design.girdle_diameter_mm = Some(6.5);
    assert_eq!(stone_width_mm(&design).to_bits(), 6.5_f64.to_bits());
    design.girdle_diameter_mm = Some(0.0);
    assert_eq!(stone_width_mm(&design).to_bits(), 7.0_f64.to_bits());
}

#[test]
fn a_saved_variant_goes_onto_the_design_as_one_edit() {
    let design = design_of("Sapphire");
    let saved = saved_uniaxial("Sapphire");
    let applied = applied_variant(&design.material, &saved).expect("an edit");
    let choice = applied.color_variant_override.as_ref().expect("a variant");
    assert_eq!(choice.key, saved.key);
    assert_eq!(choice.label, saved.label);
    assert!(choice.absorption.is_pleochroic);
    assert!(
        applied_variant(&applied, &saved).is_none(),
        "the design already uses it: no second undo step"
    );
    // The editor that saved it rests on what the design now stores.
    let host = host_material(&design, &[]).expect("Sapphire resolves");
    let mut editor = AxisEditor::open(&design.material, Some(host), 7.0, 5.0);
    solve_all(&mut editor);
    editor.settle();
    assert!(editor.applied_to(&applied).is_none());
    editor.set_active_lch([30.0, 20.0, 40.0]);
    solve_all(&mut editor);
    assert!(editor.applied_to(&applied).is_some());
}

#[test]
fn a_nameless_design_gets_the_plain_list_and_no_variant() {
    let mut design = design_of("Sapphire");
    design.material.name = None;
    assert!(host_material(&design, &[]).is_none());
    let colors = DesignColors::for_design(None, &design, &[], &[]);
    assert!(colors.host.is_none());
    assert_eq!(
        variant_rows(&colors),
        0,
        "no variant on a design with no material"
    );
    assert_eq!(presets(&colors), BODY_COLOR_PRESETS.len());
    let blank = design_of("  ");
    assert!(
        host_material(&blank, &[]).is_none(),
        "a blank name is no name"
    );
}

#[test]
fn the_cached_list_follows_the_legacy_bands_of_the_design() {
    let triple = BODY_COLOR_PRESETS[1].absorption_rgb;
    let mut with_triple = design_of("Sapphire");
    with_triple.material.body_color_override = Some(triple);
    let mut with_bands = with_triple.clone();
    let blue = solve_lch([50.0, 30.0, 265.0], 5.0, &AtomicBool::new(false)).expect("solves");
    with_bands.material.body_color_bands_override = Some(blue.bands);
    let shown = DesignColors::for_design(None, &with_bands, &[], &[]);
    assert!(
        shown
            .items
            .iter()
            .all(|item| !item.label.starts_with("Legacy:")),
        "no Legacy row while the old bands are stored"
    );
    let next = DesignColors::for_design(Some(&shown), &with_triple, &[], &[]);
    assert!(
        next.items
            .iter()
            .any(|item| item.label.starts_with("Legacy:")),
        "the triple alone shows its Legacy row, not the cached list of the banded design"
    );
}
