use super::*;

#[test]
fn record_recent_native_file_inserts_at_the_front() {
    let mut settings = AppSettings::default();
    settings.record_recent_native_file("a.psdd".to_string());
    settings.record_recent_native_file("b.psdd".to_string());
    assert_eq!(
        settings.recent_native_files,
        vec!["b.psdd".to_string(), "a.psdd".to_string()]
    );
}

#[test]
fn record_recent_native_file_moves_an_existing_entry_to_the_front_without_duplicating() {
    let mut settings = AppSettings::default();
    settings.record_recent_native_file("a.psdd".to_string());
    settings.record_recent_native_file("b.psdd".to_string());
    settings.record_recent_native_file("a.psdd".to_string());
    assert_eq!(
        settings.recent_native_files,
        vec!["a.psdd".to_string(), "b.psdd".to_string()]
    );
}

#[test]
fn record_recent_native_file_caps_at_max_recent_native_files() {
    let mut settings = AppSettings::default();
    for i in 0..(MAX_RECENT_NATIVE_FILES + 3) {
        settings.record_recent_native_file(format!("design-{i}.psdd"));
    }
    assert_eq!(settings.recent_native_files.len(), MAX_RECENT_NATIVE_FILES);
    // Most recent first: the last one recorded is still at the front, and the
    // oldest entries fell off the back rather than the front.
    assert_eq!(
        settings.recent_native_files[0],
        format!("design-{}.psdd", MAX_RECENT_NATIVE_FILES + 2)
    );
}

// --- Suppressed confirmations: AppSettings::suppressed_confirmations ---

#[test]
fn a_fresh_settings_file_suppresses_nothing() {
    let settings = AppSettings::default();
    assert!(!settings.is_confirm_suppressed("write_confirm.not_closed_solid"));
}

#[test]
fn suppress_confirm_is_reflected_immediately() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    assert!(settings.is_confirm_suppressed("write_confirm.not_closed_solid"));
}

#[test]
fn suppressing_one_key_does_not_suppress_a_different_one() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    assert!(!settings.is_confirm_suppressed("write_confirm.overwrite_unrelated"));
}

#[test]
fn suppress_confirm_is_idempotent() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    settings.suppress_confirm("write_confirm.not_closed_solid");
    assert_eq!(settings.suppressed_confirmations.len(), 1);
}

/// A round trip through TOML -- the actual persistence mechanism
/// (`settings::store`) -- so a regression that drops `#[serde(default)]` or
/// breaks `BTreeSet<String>` serialisation is caught here rather than only in
/// a live app.
#[test]
fn suppressed_confirmations_round_trips_through_toml() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    settings.suppress_confirm("write_confirm.overwrite_unrelated");
    let toml_text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(
        restored.suppressed_confirmations,
        settings.suppressed_confirmations
    );
}

/// A settings file saved BEFORE this field existed (no `suppressed_confirmations`
/// key at all) must still load, with nothing suppressed -- the idempotent
/// migration this field's own `#[serde(default)]` provides.
#[test]
fn a_settings_file_predating_this_field_loads_with_nothing_suppressed() {
    let settings = AppSettings::default();
    let mut toml_text = toml::to_string(&settings).expect("settings must serialize");
    // Strip the field this test is specifically about, simulating an
    // old-format file that never had it.
    let filtered: String = toml_text
        .lines()
        .filter(|line| !line.contains("suppressed_confirmations"))
        .collect::<Vec<_>>()
        .join("\n");
    toml_text = filtered;
    let restored: AppSettings =
        toml::from_str(&toml_text).expect("a settings file missing this field must still parse");
    assert!(restored.suppressed_confirmations.is_empty());
}

// --- Live Render colour: AppSettings::render_body_color_override ---

/// The yellow body-colour preset's absorption triple: values with a fractional part that
/// does not survive a lossy text round trip, so a bit-exact comparison is meaningful.
const YELLOW: [f32; 3] = [0.2, 0.4, 2.8];

/// Loads a settings document that carries `value` for the key and nothing else changed.
fn load_with_color_value(value: &str) -> AppSettings {
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    toml::from_str(&format!("render_body_color_override = {value}\n{text}"))
        .expect("a bad colour value must not fail the whole settings file")
}

#[test]
fn the_render_colour_defaults_to_the_material_default() {
    assert_eq!(AppSettings::default().render_body_color_override, None);
}

#[test]
fn the_render_colour_round_trips_through_toml_bit_for_bit() {
    let settings = AppSettings {
        render_body_color_override: Some(YELLOW),
        ..AppSettings::default()
    };
    let toml_text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, Some(YELLOW));

    // "Material default" writes no key at all and reads back as None.
    let cleared = AppSettings {
        render_body_color_override: None,
        ..settings
    };
    let toml_text = toml::to_string(&cleared).expect("settings must serialize");
    assert!(!toml_text.contains("render_body_color_override"));
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, None);
}

#[test]
fn a_settings_file_predating_the_render_colour_loads_with_the_material_default() {
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, None);
}

#[test]
fn a_hand_edited_render_colour_that_is_not_three_numbers_loads_as_the_default() {
    for bad in [
        "[1.0, 2.0]",
        "[1.0, 2.0, 3.0, 4.0]",
        "\"yellow\"",
        "[\"a\", \"b\", \"c\"]",
        "42",
    ] {
        assert_eq!(
            load_with_color_value(bad).render_body_color_override,
            None,
            "value {bad}"
        );
    }
    assert_eq!(
        load_with_color_value("[nan, 0.0, 0.0]").render_body_color_override,
        None,
        "a non-finite channel must not reach the renderer"
    );
}

#[test]
fn a_hand_edited_render_colour_is_limited_to_the_supported_range() {
    let loaded = load_with_color_value("[50.0, -1.0, 2.0]");
    assert_eq!(
        loaded.render_body_color_override,
        Some([MAX_RENDER_BODY_COLOR_CHANNEL, 0.0, 2.0])
    );
}

/// Loads a settings document that carries `value` for the band-rows key and nothing else
/// changed.
fn load_with_band_rows(value: &str) -> AppSettings {
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    toml::from_str(&format!("render_body_color_bands = {value}\n{text}"))
        .expect("a bad band list must not fail the whole settings file")
}

#[test]
fn the_render_colour_band_rows_default_to_none_and_round_trip_bit_for_bit() {
    assert_eq!(AppSettings::default().render_body_color_bands, None);

    let rows = vec![
        [460.0f32, 45.0, 0.2],
        [580.0, 60.0, 0.4],
        [650.0, 35.0, 2.8],
    ];
    let settings = AppSettings {
        render_body_color_override: Some(YELLOW),
        render_body_color_bands: Some(rows.clone()),
        ..AppSettings::default()
    };
    let toml_text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, Some(YELLOW));
    assert_eq!(restored.render_body_color_bands, Some(rows));

    // A color that is the triple alone writes no band key at all and reads back as None.
    let triple_only = AppSettings {
        render_body_color_bands: None,
        ..settings
    };
    let toml_text = toml::to_string(&triple_only).expect("settings must serialize");
    assert!(!toml_text.contains("render_body_color_bands"));
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, Some(YELLOW));
    assert_eq!(restored.render_body_color_bands, None);
}

#[test]
fn a_settings_file_predating_the_band_rows_still_loads_with_the_triple() {
    // The band key is absent, as in every file written before the rows were saved.
    let old = AppSettings {
        render_body_color_override: Some(YELLOW),
        ..AppSettings::default()
    };
    let text = toml::to_string(&old).expect("settings must serialize");
    assert!(!text.contains("render_body_color_bands"));
    let restored: AppSettings = toml::from_str(&text).expect("an old file must parse");
    assert_eq!(restored.render_body_color_override, Some(YELLOW));
    assert_eq!(restored.render_body_color_bands, None);
}

#[test]
fn hand_edited_band_rows_keep_only_the_usable_rows() {
    assert_eq!(
        load_with_band_rows("[[460.0, 45.0, 0.25], [0.0, 45.0, 0.25], [500.0, -1.0, 0.25], [1.0, 2.0], [nan, 45.0, 0.25]]")
            .render_body_color_bands,
        Some(vec![[460.0, 45.0, 0.25]]),
        "rows that are not finite and positive are dropped"
    );
    for bad in [
        "\"blue\"",
        "42",
        "[]",
        "[[0.0, 0.0, 0.0]]",
        "[\"a\", \"b\"]",
    ] {
        assert_eq!(
            load_with_band_rows(bad).render_body_color_bands,
            None,
            "value {bad}"
        );
    }
    let many = format!("[{}]", vec!["[500.0, 40.0, 0.5]"; 40].join(", "));
    assert_eq!(
        load_with_band_rows(&many)
            .render_body_color_bands
            .map(|rows| rows.len()),
        Some(MAX_RENDER_BODY_COLOR_BANDS)
    );
}

// --- Live Render color variant: AppSettings::render_body_color_variant ---

/// A pleochroic uniaxial choice with awkward float values and both band shapes, so a bit-exact
/// comparison means something.
fn view_variant_choice() -> VariantChoice {
    use pleochroma::optics::absorption::{AbsorptionBand, AbsorptionTensor};
    VariantChoice {
        key: "sapphire.padparadscha".to_owned(),
        label: "Padparadscha".to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![
                AbsorptionBand::new(570.123, 41.7, 0.034_567),
                AbsorptionBand::energy(451.1, 1500.5, 0.000_123),
            ],
            vec![AbsorptionBand::new(430.0, 38.3333, 0.12)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.25,
    }
}

/// A variant as the inline TOML table the settings file holds, built from the given unit
/// spelling, reference path and band rows (each ray's rows as inline-table text), so a test can
/// break exactly one value.
fn variant_toml(unit: &str, reference_path_mm: &str, o_ray: &str, e_ray: &str) -> String {
    format!(
        "{{ key = \"sapphire.test\", label = \"Test\", unit = \"{unit}\", \
         reference_path_mm = {reference_path_mm}, \
         tensor = {{ o_ray = [{o_ray}], e_ray = [{e_ray}], pleochroic = true }} }}"
    )
}

const GOOD_ROW: &str = "{ center_nm = 550.0, width_nm = 40.0, peak = 0.3, shape = \"wavelength\" }";

/// Loads a settings document that carries `value` for the variant key and nothing else changed.
fn load_with_variant(value: &str) -> AppSettings {
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    toml::from_str(&format!("render_body_color_variant = {value}\n{text}"))
        .expect("a bad variant must not fail the whole settings file")
}

#[test]
fn the_render_color_variant_defaults_to_none_and_a_file_without_it_loads() {
    assert_eq!(AppSettings::default().render_body_color_variant, None);
    assert_eq!(AppSettings::default().render_view_variant(), None);
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    assert!(!text.contains("render_body_color_variant"));
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(restored.render_body_color_variant, None);
}

/// A file that has only the legacy keys (every file written before variants) loads them as
/// saved, with no variant.
#[test]
fn a_settings_file_with_only_the_old_color_fields_loads_them_unchanged() {
    let rows = vec![[460.0f32, 45.0, 0.2], [580.0, 60.0, 0.4]];
    let old = AppSettings {
        render_body_color_override: Some(YELLOW),
        render_body_color_bands: Some(rows.clone()),
        ..AppSettings::default()
    };
    let text = toml::to_string(&old).expect("settings must serialize");
    assert!(!text.contains("render_body_color_variant"));
    let restored: AppSettings = toml::from_str(&text).expect("an old file must parse");
    assert_eq!(restored.render_body_color_override, Some(YELLOW));
    assert_eq!(restored.render_body_color_bands, Some(rows));
    assert_eq!(restored.render_body_color_variant, None);
    assert_eq!(restored.render_view_variant(), None);
}

#[test]
fn the_render_color_variant_round_trips_through_toml_bit_for_bit() {
    let choice = view_variant_choice();
    let mut settings = AppSettings::default();
    settings.set_render_view_variant(Some(("Sapphire", &choice)));
    let text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(
        restored.render_body_color_variant,
        settings.render_body_color_variant
    );

    let (material, back) = restored.render_view_variant().expect("a stored variant");
    assert_eq!(material, "Sapphire");
    assert_eq!(back, choice);
    assert_eq!(
        back.reference_path_mm.to_bits(),
        choice.reference_path_mm.to_bits()
    );
    for (a, b) in back
        .absorption
        .o_ray
        .iter()
        .chain(&back.absorption.e_ray)
        .zip(
            choice
                .absorption
                .o_ray
                .iter()
                .chain(&choice.absorption.e_ray),
        )
    {
        assert_eq!(a.center_nm.to_bits(), b.center_nm.to_bits());
        assert_eq!(a.width_nm.to_bits(), b.width_nm.to_bits());
        assert_eq!(a.peak.to_bits(), b.peak.to_bits());
        assert_eq!(a.shape, b.shape, "the band shape is stored, not inferred");
    }
    assert!(back.absorption.is_pleochroic);

    // Forgetting it writes no key and reads back as None.
    settings.set_render_view_variant(None);
    let text = toml::to_string(&settings).expect("settings must serialize");
    assert!(!text.contains("render_body_color_variant"));
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(restored.render_view_variant(), None);
}

#[test]
fn a_biaxial_and_a_per_model_unit_variant_round_trip() {
    use pleochroma::optics::absorption::{AbsorptionBand, AbsorptionTensor};
    let choice = VariantChoice {
        key: "custom".to_owned(),
        label: "Custom Color".to_owned(),
        absorption: AbsorptionTensor::biaxial(
            vec![AbsorptionBand::new(600.7, 55.55, 0.31)],
            vec![AbsorptionBand::energy(520.3, 2100.1, 0.077)],
            vec![AbsorptionBand::new(455.5, 30.3, 0.6)],
        ),
        unit: AbsorptionUnit::ModelUnit,
        reference_path_mm: 5.0,
    };
    let mut settings = AppSettings::default();
    settings.set_render_view_variant(Some(("", &choice)));
    let text = toml::to_string(&settings).expect("settings must serialize");
    assert!(
        text.contains("model_unit"),
        "the unit is spelled out: {text}"
    );
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(
        restored.render_view_variant(),
        Some((String::new(), choice))
    );
}

#[test]
fn a_usable_hand_written_variant_loads_and_a_missing_material_is_empty() {
    let loaded = load_with_variant(&variant_toml("per_mm", "6.0", GOOD_ROW, GOOD_ROW));
    let (material, choice) = loaded.render_view_variant().expect("a usable variant");
    assert_eq!(material, "");
    assert_eq!(choice.key, "sapphire.test");
    assert_eq!(choice.unit, AbsorptionUnit::PerMm);
    assert_eq!(choice.absorption.o_ray.len(), 1);
}

#[test]
fn a_hand_edited_variant_that_cannot_be_used_loads_as_none() {
    let nine = [GOOD_ROW; 9].join(", ");
    let cases = [
        ("a string", "\"blue\"".to_owned()),
        ("a number", "42".to_owned()),
        (
            "an unknown unit",
            variant_toml("ghost", "6.0", GOOD_ROW, GOOD_ROW),
        ),
        (
            "no reference path",
            variant_toml("per_mm", "0.0", GOOD_ROW, GOOD_ROW),
        ),
        (
            "a huge reference path",
            variant_toml("per_mm", "5000.0", GOOD_ROW, GOOD_ROW),
        ),
        (
            "a negative peak",
            variant_toml(
                "per_mm",
                "6.0",
                "{ center_nm = 550.0, width_nm = 40.0, peak = -0.3, shape = \"wavelength\" }",
                GOOD_ROW,
            ),
        ),
        (
            "a NaN width",
            variant_toml(
                "per_mm",
                "6.0",
                "{ center_nm = 550.0, width_nm = nan, peak = 0.3, shape = \"wavelength\" }",
                GOOD_ROW,
            ),
        ),
        (
            "an unknown band shape",
            variant_toml(
                "per_mm",
                "6.0",
                "{ center_nm = 550.0, width_nm = 40.0, peak = 0.3, shape = \"cubic\" }",
                GOOD_ROW,
            ),
        ),
        (
            "nine bands in one ray",
            variant_toml("per_mm", "6.0", &nine, GOOD_ROW),
        ),
        (
            "one ray empty, the other not",
            variant_toml("per_mm", "6.0", "", GOOD_ROW),
        ),
    ];
    for (what, value) in cases {
        assert_eq!(
            load_with_variant(&value).render_body_color_variant,
            None,
            "{what}"
        );
    }
    // A colorless variant (every ray empty) is valid.
    assert!(
        load_with_variant(&variant_toml("per_mm", "6.0", "", ""))
            .render_body_color_variant
            .is_some()
    );
}

#[test]
fn an_unusable_variant_does_not_disturb_the_legacy_color_beside_it() {
    let text = toml::to_string(&AppSettings {
        render_body_color_override: Some(YELLOW),
        ..AppSettings::default()
    })
    .expect("settings must serialize");
    let loaded: AppSettings = toml::from_str(&format!("render_body_color_variant = 42\n{text}"))
        .expect("a bad variant must not fail the file");
    assert_eq!(loaded.render_body_color_variant, None);
    assert_eq!(loaded.render_body_color_override, Some(YELLOW));
}

#[test]
fn a_settings_file_without_the_update_table_is_not_asked_yet() {
    let parsed: crate::settings::model::SettingsFile =
        toml::from_str("[settings]\nexposure = 1.1\n").expect("deserialize");
    assert!(parsed.settings.update_check.needs_consent());
    assert_eq!(parsed.settings.update_check.last_check_unix, 0);
}

#[test]
fn the_update_check_record_round_trips_through_toml() {
    let mut file = crate::settings::model::SettingsFile::default();
    file.settings.update_check = pleochroma_update::UpdatePreferences {
        enabled: Some(true),
        last_check_unix: 1_760_000_000,
        skipped_version: Some("0.7.3".to_owned()),
    };
    let text = toml::to_string_pretty(&file).expect("serialize");
    let parsed: crate::settings::model::SettingsFile = toml::from_str(&text).expect("deserialize");
    assert_eq!(parsed.settings.update_check, file.settings.update_check);
}
#[test]
fn legacy_settings_open_without_roll_and_new_settings_keep_the_image_orientation() {
    let legacy: super::AppSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.camera_roll, 0.0);
    let rolled = super::AppSettings {
        camera_roll: std::f32::consts::FRAC_PI_2,
        ..legacy
    };
    let restored: super::AppSettings =
        serde_json::from_str(&serde_json::to_string(&rolled).unwrap()).unwrap();
    assert_eq!(restored.camera_roll, rolled.camera_roll);
}
