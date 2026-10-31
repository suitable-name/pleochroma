use super::*;
use pleochroma::optics::{
    absorption::AbsorptionBand,
    materials::{AbsorptionUnit, variants::tensor_rank},
};
use pleochroma_cut_core::material::CUSTOM_VARIANT_LABEL;

fn named(name: &str) -> MaterialSelection {
    MaterialSelection {
        name: Some(name.to_owned()),
        ..MaterialSelection::none()
    }
}

/// A pleochroic saved variant of `material`.
fn user_variant(material: &str, key: &str, label: &str) -> ColorVariant {
    ColorVariant {
        key: key.to_owned(),
        label: label.to_owned(),
        material: material.to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(540.0, 3000.0, 0.6)],
            vec![AbsorptionBand::energy(450.0, 3000.0, 0.3)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
        provenance: VariantProvenance::User,
    }
}

/// A Custom Color snapshot of a uniaxial tensor.
fn custom_choice() -> VariantChoice {
    VariantChoice {
        key: CUSTOM_VARIANT_KEY.to_owned(),
        label: CUSTOM_VARIANT_LABEL.to_owned(),
        absorption: user_variant("Sapphire", "x", "x").absorption,
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 5.0,
    }
}

fn variant_keys(items: &[ColorEntryItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match &item.entry {
            ColorEntry::Variant { key } => Some(key.clone()),
            _ => None,
        })
        .collect()
}

fn is_preset(item: &ColorEntryItem) -> bool {
    matches!(item.entry, ColorEntry::Preset(_))
}

#[test]
fn a_pleochroic_host_lists_its_variants_and_no_generic_presets() {
    let sapphire = GemMaterial::sapphire();
    let items = color_entries(&sapphire, &[]);
    assert_eq!(items[0].entry, ColorEntry::MaterialDefault);
    assert_eq!(items[0].label, "Material Default");
    assert_eq!(
        items.last().map(|item| &item.entry),
        Some(&ColorEntry::Custom)
    );
    assert!(
        !items.iter().any(is_preset),
        "the nine isotropic presets are not offered for a pleochroic host"
    );
    let built_in: Vec<String> = built_in_variants("Sapphire")
        .into_iter()
        .skip(1)
        .map(|variant| variant.key)
        .collect();
    assert!(!built_in.is_empty(), "Sapphire ships color variants");
    assert_eq!(variant_keys(&items), built_in, "in the data file's order");
}

#[test]
fn an_isotropic_host_lists_the_nine_presets_before_custom() {
    let diamond = GemMaterial::diamond();
    let items = color_entries(&diamond, &[]);
    assert_eq!(items[0].entry, ColorEntry::MaterialDefault);
    let presets: Vec<usize> = items
        .iter()
        .filter_map(|item| match item.entry {
            ColorEntry::Preset(index) => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(presets, (0..BODY_COLOR_PRESETS.len()).collect::<Vec<_>>());
    let first_preset = items.iter().position(is_preset).expect("a preset");
    let last_variant = items
        .iter()
        .rposition(|item| matches!(item.entry, ColorEntry::Variant { .. }));
    assert!(last_variant.is_none_or(|at| at < first_preset));
    assert_eq!(
        items.last().map(|item| &item.entry),
        Some(&ColorEntry::Custom)
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item.entry == ColorEntry::Custom)
            .count(),
        1
    );
}

#[test]
fn saved_variants_follow_the_built_in_ones_and_only_for_their_material() {
    let sapphire = GemMaterial::sapphire();
    let mine = user_variant("Sapphire", "user.1", "Mine");
    let other_material = user_variant("Ruby", "user.2", "Rubies Only");
    // A saved variant the host does not accept: an isotropic color for a pleochroic host.
    let mut flat = user_variant("Sapphire", "user.3", "Flat");
    flat.absorption = AbsorptionTensor::isotropic(flat.absorption.o_ray.clone());
    let items = color_entries(&sapphire, &[mine, other_material, flat]);
    let keys = variant_keys(&items);
    let built_in = keys.iter().filter(|key| !key.starts_with("user.")).count();
    assert_eq!(keys.len(), built_in + 1, "only the fitting saved variant");
    assert_eq!(keys[built_in], "user.1", "after every built-in one");
    let at = items
        .iter()
        .position(|item| item.label == "Mine")
        .expect("listed");
    assert!(items[at].tooltip.ends_with("Saved by you."));
    assert!(items.iter().all(|item| item.label != "Rubies Only"));
    assert!(items.iter().all(|item| item.label != "Flat"));
    assert!(at < items.len() - 1, "Custom stays last");
}

#[test]
fn every_row_but_custom_has_a_swatch_and_every_row_a_tooltip() {
    for host in [GemMaterial::sapphire(), GemMaterial::diamond()] {
        for item in color_entries(&host, &[]) {
            assert!(!item.tooltip.is_empty(), "{}", item.label);
            assert!(!item.tooltip.contains("colour"), "{}", item.label);
            if item.entry == ColorEntry::Custom {
                assert_eq!(item.swatch_hex, "");
                assert_eq!(item.swatch_rgb(), None);
            } else {
                let rgb = item.swatch_rgb().expect("a #rrggbb swatch");
                assert_eq!(hex_from_rgb(rgb), item.swatch_hex, "{}", item.label);
            }
        }
    }
}

#[test]
fn a_pleochroic_tooltip_names_the_color_of_each_direction() {
    let items = color_entries(&GemMaterial::sapphire(), &[]);
    assert!(
        items[0].tooltip.contains("Along the Axis: "),
        "{}",
        items[0].tooltip
    );
    assert!(items[0].tooltip.contains(" / Across the Axis: "));
    let diamond = color_entries(&GemMaterial::diamond(), &[]);
    assert!(diamond[0].tooltip.contains("Face-up color: "));
}

/// A built-in provenance with the given sources and `estimated` flag.
fn built_in(sources: &[&str], estimated: bool) -> VariantProvenance {
    VariantProvenance::BuiltIn {
        sources: sources.iter().map(|s| (*s).to_owned()).collect(),
        fit_record: None,
        estimated,
    }
}

#[test]
fn an_estimated_built_in_names_its_first_source_line() {
    let note = estimated_note(&built_in(
        &["Derived from the built-in Ruby tensor; estimated", "Second"],
        true,
    ));
    assert_eq!(
        note.as_deref(),
        Some("Estimated: Derived from the built-in Ruby tensor; estimated")
    );
    // A multi-line source gives its first non-empty line only.
    let multi = estimated_note(&built_in(&["\n  First line\nSecond line"], true));
    assert_eq!(multi.as_deref(), Some("Estimated: First line"));
    // No usable source still says it is an estimate.
    let unsourced = estimated_note(&built_in(&[], true));
    assert_eq!(
        unsourced.as_deref(),
        Some("Estimated: no published spectrum.")
    );
}

#[test]
fn only_an_estimated_built_in_has_the_estimated_note() {
    assert_eq!(estimated_note(&built_in(&["Rossman"], false)), None);
    assert_eq!(estimated_note(&VariantProvenance::User), None);
    assert_eq!(estimated_note(&VariantProvenance::MaterialDefault), None);
}

#[test]
fn an_estimated_variant_row_carries_the_note_in_its_tooltip() {
    let mut estimated = user_variant("Sapphire", "sapphire.test", "Test");
    estimated.provenance = built_in(&["Fe3+ bands of the corundum host; estimated"], true);
    let item = variant_item(&GemMaterial::sapphire(), &estimated);
    assert!(
        item.tooltip
            .ends_with("Estimated: Fe3+ bands of the corundum host; estimated"),
        "{}",
        item.tooltip
    );
    assert!(!item.tooltip.contains("Saved by you"));
}

/// The sRGB swatch of `tensor` at the optical path `path_mm`, unpolarised (a one-color tensor's
/// face-up color).
fn swatch_at(tensor: &AbsorptionTensor, path_mm: f64) -> String {
    hex_from_rgb(
        body_colors(tensor, path_mm, Illuminant::D65)
            .unpolarised
            .srgb,
    )
}

/// The swatch of a variant row: the oriented face-up color (`face_up_swatch`) for the stone
/// width the entry is calibrated for. 7 mm is the stone width, not the optical path.
#[test]
fn swatches_are_the_oriented_face_up_color_at_the_calibrated_stone_width() {
    // The material's own color, in the material's own unit, for a 7 mm stone.
    for host in [GemMaterial::sapphire(), GemMaterial::diamond()] {
        let items = color_entries(&host, &[]);
        let expected = face_up_swatch(&host.absorption, host.absorption_unit, 7.0, &host);
        assert_eq!(
            items[0].swatch_hex,
            hex_from_rgb(expected.srgb),
            "{}",
            host.name
        );
    }
    // A per-mm variant at the stone width it carries (its reference_path_mm is a width).
    let per_mm = face_up_swatch_path(AbsorptionUnit::PerMm, 7.0);
    // 2.83 x 7 / 2 from f32 constants is 9.904999733 in f64: an f32-sized tolerance.
    assert!((per_mm - 9.905).abs() < 1e-6, "{per_mm}");
    let sapphire = GemMaterial::sapphire();
    let mut light = user_variant("Sapphire", "user.1", "Mine");
    light.absorption = AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::energy(540.0, 3000.0, 0.03)],
        vec![AbsorptionBand::energy(450.0, 3000.0, 0.02)],
    );
    let swatch_of = |variant: &ColorVariant| {
        let items = color_entries(&sapphire, std::slice::from_ref(variant));
        let at = items
            .iter()
            .position(|item| item.label == "Mine")
            .expect("listed");
        items[at].swatch_hex.clone()
    };
    for width in [7.0_f32, 10.0] {
        light.reference_path_mm = width;
        let expected = face_up_swatch(&light.absorption, light.unit, f64::from(width), &sapphire);
        assert_eq!(swatch_of(&light), hex_from_rgb(expected.srgb), "{width} mm");
    }
    light.reference_path_mm = 7.0;
    let seven = swatch_of(&light);
    light.reference_path_mm = 14.0;
    assert_ne!(
        seven,
        swatch_of(&light),
        "a variant calibrated for a wider stone is shown deeper"
    );
    // A generic preset is a per-model-unit one-color tensor: its unpolarised color at one unit.
    let diamond = color_entries(&GemMaterial::diamond(), &[]);
    let preset = diamond
        .iter()
        .find(|item| item.entry == ColorEntry::Preset(2))
        .expect("a preset");
    let tensor =
        AbsorptionTensor::isotropic(legacy_rgb_bands(BODY_COLOR_PRESETS[2].absorption_rgb));
    assert_eq!(preset.swatch_hex, swatch_at(&tensor, 1.0));
}

/// A one-color tensor has no orientation: its swatch is the unpolarised color.
#[test]
fn a_one_color_variants_swatch_is_its_unpolarised_color() {
    let diamond = GemMaterial::diamond();
    let mut flat = user_variant("Diamond", "user.flat", "Flat");
    flat.absorption =
        AbsorptionTensor::isotropic(vec![AbsorptionBand::energy(450.0, 3000.0, 0.03)]);
    let items = color_entries(&diamond, std::slice::from_ref(&flat));
    let at = items
        .iter()
        .position(|item| item.label == "Flat")
        .expect("listed");
    assert_eq!(
        items[at].swatch_hex,
        swatch_at(&flat.absorption, face_up_swatch_path(flat.unit, 7.0))
    );
}

#[test]
fn hex_swatches_parse_back_and_bad_text_is_refused() {
    assert_eq!(rgb_from_hex("#0a80ff"), Some([10, 128, 255]));
    assert_eq!(hex_from_rgb([10, 128, 255]), "#0a80ff");
    for bad in ["", "0a80ff", "#0a80f", "#0a80ffff", "#0a80fg", "#+a80ff"] {
        assert_eq!(rgb_from_hex(bad), None, "{bad:?}");
    }
}

/// Every row but Custom maps to the same row after a pick: selection <-> entry round trips.
fn assert_round_trip(host: &GemMaterial) {
    let variants = listed_variants(host, &[]);
    let items = color_entries(host, &[]);
    let base = named(&host.name);
    for (at, item) in items.iter().enumerate() {
        if item.entry == ColorEntry::Custom {
            continue;
        }
        let picked = selection_with_entry(base.clone(), &item.entry, &base, &variants);
        assert_eq!(
            entry_for_selection(&picked, &items),
            at,
            "{} on {}",
            item.label,
            host.name
        );
    }
}

#[test]
fn a_pick_and_the_row_it_shows_round_trip_for_every_kind() {
    assert_round_trip(&GemMaterial::sapphire());
    assert_round_trip(&GemMaterial::diamond());
}

#[test]
fn a_variant_row_is_a_snapshot_that_keeps_the_hosts_pleochroism() {
    let sapphire = GemMaterial::sapphire();
    let variants = listed_variants(&sapphire, &[]);
    let first = variants.first().expect("Sapphire ships variants");
    let current = named("Sapphire").with_body_color(Some([0.2, 0.4, 2.8]));
    let picked = selection_with_entry(
        named("Sapphire"),
        &ColorEntry::Variant {
            key: first.key.clone(),
        },
        &current,
        &variants,
    );
    let choice = picked.color_variant_override.as_ref().expect("a snapshot");
    assert_eq!(*choice, VariantChoice::from_variant(first));
    assert_eq!(picked.body_color_override, None, "the legacy color is gone");
    let gem = picked.apply_overrides(sapphire.clone());
    assert!(gem.absorption.is_pleochroic);
    assert_eq!(gem.name, sapphire.name);
    assert_eq!(gem.dispersion, sapphire.dispersion);
}

#[test]
fn a_plain_pick_replaces_the_stones_pattern() {
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
    let pattern = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
    let current = named("Diamond").with_color_pattern(Some(pattern));
    let blue = selection_with_entry(named("Diamond"), &ColorEntry::Preset(1), &current, &[]);
    assert_eq!(blue.color_pattern_override, None);
    assert_eq!(
        blue.body_color_override,
        Some(BODY_COLOR_PRESETS[1].absorption_rgb)
    );
    let default = selection_with_entry(
        named("Diamond"),
        &ColorEntry::MaterialDefault,
        &current,
        &[],
    );
    assert_eq!(default, named("Diamond"));
    // Custom keeps what the design has, the pattern included.
    let kept = selection_with_entry(named("Diamond"), &ColorEntry::Custom, &current, &[]);
    assert_eq!(kept.color_pattern_override, current.color_pattern_override);
}

#[test]
fn custom_keeps_a_variant_while_the_material_stays_and_drops_it_with_the_material() {
    let items = color_entries(&GemMaterial::sapphire(), &[]);
    let current = named("Sapphire").with_color_variant(Some(custom_choice()));
    // The Custom Color of the design shows as the Custom row.
    assert_eq!(
        items[entry_for_selection(&current, &items)].entry,
        ColorEntry::Custom
    );
    let kept = selection_with_entry(named("Sapphire"), &ColorEntry::Custom, &current, &[]);
    assert_eq!(kept.color_variant_override, Some(custom_choice()));
    // A species change: the variant of Sapphire has no business on Diamond.
    let switched = selection_with_entry(named("Diamond"), &ColorEntry::Custom, &current, &[]);
    assert_eq!(switched.color_variant_override, None);
    assert_eq!(switched.body_color_override, None);
}

#[test]
fn a_variant_pick_on_another_material_falls_back_to_that_materials_own_color() {
    let sapphire = GemMaterial::sapphire();
    let variants = listed_variants(&sapphire, &[]);
    let first = variants.first().expect("Sapphire ships variants");
    let current = named("Sapphire");
    let switched = selection_with_entry(
        named("Diamond"),
        &ColorEntry::Variant {
            key: first.key.clone(),
        },
        &current,
        &variants,
    );
    assert_eq!(switched, named("Diamond"));
}

#[test]
fn a_key_the_list_does_not_hold_keeps_the_current_color() {
    let current = named("Sapphire").with_color_variant(Some(custom_choice()));
    let kept = selection_with_entry(
        named("Sapphire"),
        &ColorEntry::Variant {
            key: "user.gone".to_owned(),
        },
        &current,
        &[],
    );
    assert_eq!(kept.color_variant_override, Some(custom_choice()));
}

#[test]
fn a_variant_that_is_not_listed_shows_as_custom() {
    let items = color_entries(&GemMaterial::sapphire(), &[]);
    let mut stale = custom_choice();
    stale.key = "user.deleted".to_owned();
    let selection = named("Sapphire").with_color_variant(Some(stale));
    assert_eq!(
        items[entry_for_selection(&selection, &items)].entry,
        ColorEntry::Custom
    );
}

#[test]
fn legacy_colors_show_as_custom_or_their_preset_and_none_as_the_default() {
    let diamond = GemMaterial::diamond();
    let items = color_entries(&diamond, &[]);
    assert_eq!(entry_for_selection(&named("Diamond"), &items), 0);
    let picked = named("Diamond").with_body_color(Some([0.123, 0.456, 0.789]));
    assert_eq!(
        items[entry_for_selection(&picked, &items)].entry,
        ColorEntry::Custom
    );
    let bands = named("Diamond").with_body_color_bands(
        Some([0.2, 0.4, 2.8]),
        Some(vec![[460.0, 3000.0, 0.25]]),
        Some(1.0),
    );
    assert_eq!(
        items[entry_for_selection(&bands, &items)].entry,
        ColorEntry::Custom
    );
}

#[test]
fn a_preset_triple_on_a_pleochroic_host_shows_as_the_legacy_entry() {
    let sapphire = GemMaterial::sapphire();
    let blue = BODY_COLOR_PRESETS[1];
    assert_eq!(blue.label, "Blue");
    let current = named("Sapphire").with_body_color(Some(blue.absorption_rgb));
    assert!(legacy_color_on_pleochroic_host(&current, &sapphire));

    let items = color_entries_for_design(&sapphire, &[], &current);
    let at = entry_for_selection(&current, &items);
    assert_eq!(items[at].label, "Legacy: Blue (No Pleochroism)");
    assert_eq!(items[at].entry, ColorEntry::Preset(1));
    assert_eq!(
        items.last().map(|item| &item.entry),
        Some(&ColorEntry::Custom)
    );
    assert_eq!(at, items.len() - 2, "just before Custom");
    // The plain list has no preset: the legacy row exists only for the design that carries it.
    assert!(!color_entries(&sapphire, &[]).iter().any(is_preset));
    // Picking the legacy row again keeps the legacy color as saved.
    let kept = selection_with_entry(named("Sapphire"), &items[at].entry, &current, &[]);
    assert_eq!(kept.body_color_override, Some(blue.absorption_rgb));

    // A plain design, a variant design and an isotropic host have no legacy row.
    assert_eq!(
        color_entries_for_design(&sapphire, &[], &named("Sapphire")).len(),
        items.len() - 1
    );
    let with_variant = named("Sapphire").with_color_variant(Some(custom_choice()));
    assert!(!legacy_color_on_pleochroic_host(&with_variant, &sapphire));
    let diamond = GemMaterial::diamond();
    let on_diamond = named("Diamond").with_body_color(Some(blue.absorption_rgb));
    assert!(!legacy_color_on_pleochroic_host(&on_diamond, &diamond));
    let diamond_items = color_entries_for_design(&diamond, &[], &on_diamond);
    assert_eq!(diamond_items.len(), color_entries(&diamond, &[]).len());
    assert_eq!(
        diamond_items[entry_for_selection(&on_diamond, &diamond_items)].label,
        "Blue"
    );
}

#[test]
fn a_legacy_n_band_color_on_a_pleochroic_host_is_custom_with_the_legacy_note() {
    let sapphire = GemMaterial::sapphire();
    let current = named("Sapphire").with_body_color_bands(
        Some([0.2, 0.4, 2.8]),
        Some(vec![[460.0, 3000.0, 0.25]]),
        Some(1.0),
    );
    assert!(legacy_color_on_pleochroic_host(&current, &sapphire));
    let items = color_entries_for_design(&sapphire, &[], &current);
    assert_eq!(
        items[entry_for_selection(&current, &items)].entry,
        ColorEntry::Custom
    );
}

/// Review finding 2 (scenario A): an Apply of the material row with the combo on the design's own
/// variant row keeps the design's snapshot, even when the library row moved since ("Variant
/// Changed: Update" is the way to refresh it).
#[test]
fn an_untouched_variant_row_keeps_the_snapshot_not_the_library_copy() {
    let held = user_variant("Sapphire", "user.1", "Mine");
    let current = named("Sapphire").with_color_variant(Some(VariantChoice::from_variant(&held)));
    let mut moved = held.clone();
    moved.absorption = AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::energy(540.0, 3000.0, 0.9)],
        vec![AbsorptionBand::energy(450.0, 3000.0, 0.1)],
    );
    let entry = ColorEntry::Variant {
        key: "user.1".to_owned(),
    };
    let applied = selection_with_entry(named("Sapphire"), &entry, &current, &[moved]);
    assert_eq!(applied, current, "nothing changes, the snapshot stays");
    assert_eq!(
        applied.color_variant_override,
        Some(VariantChoice::from_variant(&held))
    );
}

/// Review finding 2 (scenario B): a design with a variant AND a color pattern keeps the pattern
/// when the combo is left on the variant row (and on a preset row of an isotropic host).
#[test]
fn an_untouched_row_keeps_the_color_pattern() {
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
    let pattern = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
    let held = user_variant("Sapphire", "user.1", "Mine");
    let current = named("Sapphire")
        .with_color_variant(Some(VariantChoice::from_variant(&held)))
        .with_color_pattern(Some(pattern.clone()));
    let entry = ColorEntry::Variant {
        key: "user.1".to_owned(),
    };
    let applied = selection_with_entry(named("Sapphire"), &entry, &current, &[held]);
    assert_eq!(applied.color_pattern_override, Some(pattern.clone()));
    assert_eq!(
        applied.color_variant_override,
        current.color_variant_override
    );

    // A preset triple on an isotropic host, untouched: the triple and the pattern stay.
    let blue = BODY_COLOR_PRESETS[1].absorption_rgb;
    let diamond = named("Diamond")
        .with_body_color(Some(blue))
        .with_color_pattern(Some(pattern.clone()));
    let kept = selection_with_entry(named("Diamond"), &ColorEntry::Preset(1), &diamond, &[]);
    assert_eq!(kept.body_color_override, Some(blue));
    assert_eq!(kept.color_pattern_override, Some(pattern));
}

/// Picking ANOTHER row is still a pick: a different variant replaces the snapshot and the
/// pattern, and a species change never counts as "the same row".
#[test]
fn another_row_or_another_material_is_still_a_pick() {
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
    let first = user_variant("Sapphire", "user.1", "Mine");
    let second = user_variant("Sapphire", "user.2", "Other");
    let pattern = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
    let current = named("Sapphire")
        .with_color_variant(Some(VariantChoice::from_variant(&first)))
        .with_color_pattern(Some(pattern));
    let entry = ColorEntry::Variant {
        key: "user.2".to_owned(),
    };
    let picked = selection_with_entry(
        named("Sapphire"),
        &entry,
        &current,
        &[first.clone(), second.clone()],
    );
    assert_eq!(
        picked.color_variant_override,
        Some(VariantChoice::from_variant(&second))
    );
    assert_eq!(picked.color_pattern_override, None);
    // The same key on another material: that material's own color.
    let switched = selection_with_entry(
        named("Ruby"),
        &ColorEntry::Variant {
            key: "user.1".to_owned(),
        },
        &current,
        &[first],
    );
    assert_eq!(switched, named("Ruby"));
}

/// Review finding web 5: a variant pick on a design with no material name is the plain color
/// (the host of a nameless design is only a refractive-index guess).
#[test]
fn a_variant_pick_on_a_nameless_design_falls_back_to_the_plain_color() {
    let variant = user_variant("Sapphire", "user.1", "Mine");
    let nameless = MaterialSelection::none();
    let picked = selection_with_entry(
        MaterialSelection::none(),
        &ColorEntry::Variant {
            key: "user.1".to_owned(),
        },
        &nameless,
        &[variant],
    );
    assert_eq!(picked, MaterialSelection::none());
    assert_eq!(picked.color_variant_override, None);
}

/// The Material Default row of a design with no color changes nothing, but a color pattern is
/// still replaced by an explicit Material Default pick (the plain-color rule).
#[test]
fn the_material_default_row_of_a_plain_design_changes_nothing() {
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
    let plain = named("Sapphire");
    assert_eq!(
        selection_with_entry(named("Sapphire"), &ColorEntry::MaterialDefault, &plain, &[]),
        plain
    );
    let patterned = named("Sapphire").with_color_pattern(Some(template(
        TemplateId::Bicolor,
        PatternAxis::TABLE_NORMAL,
    )));
    let cleared = selection_with_entry(
        named("Sapphire"),
        &ColorEntry::MaterialDefault,
        &patterned,
        &[],
    );
    assert_eq!(cleared.color_pattern_override, None);
}

#[test]
fn no_row_of_a_pleochroic_host_gives_it_an_isotropic_tensor() {
    // Every row a pleochroic host can pick (variants only, no presets) keeps a pleochroic tensor.
    for name in ["Sapphire", "Ruby", "Alexandrite", "Tourmaline", "Tanzanite"] {
        let Some(host) = GemMaterial::built_in()
            .iter()
            .find(|material| material.name.eq_ignore_ascii_case(name))
        else {
            continue;
        };
        if !host.absorption.is_pleochroic {
            continue;
        }
        let variants = listed_variants(host, &[]);
        for item in color_entries(host, &[]) {
            let picked = selection_with_entry(
                named(&host.name),
                &item.entry,
                &named(&host.name),
                &variants,
            );
            if item.entry == ColorEntry::MaterialDefault {
                assert_eq!(picked.apply_overrides(host.clone()), host.clone());
            } else if let Some(choice) = &picked.color_variant_override {
                assert_eq!(
                    tensor_rank(&choice.absorption),
                    tensor_rank(&picked.apply_overrides(host.clone()).absorption)
                );
                // A colorless variant is an all-empty tensor: it has no pleochroism to lose.
                let tensor = &choice.absorption;
                let colorless = tensor.o_ray.is_empty()
                    && tensor.e_ray.is_empty()
                    && tensor.beta_ray.as_ref().is_none_or(Vec::is_empty);
                assert!(colorless || tensor.is_pleochroic, "{name}: {}", item.label);
            }
        }
    }
}
