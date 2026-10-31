//! The design settings panel's color combo with its trailing "Custom..." entry.
//!
//! The combo lists the material's own color, the material's color variants (built in and the
//! cutter's saved ones), the nine generic presets (for a material that looks the same in every
//! direction only) and a last "Custom..." entry: the keyed list of
//! [`pleochroma_editor::material::color_entries_for_design`]. Choosing "Custom..." opens the
//! color editor under the combo (one color row, or one per direction for a pleochroic stone,
//! `render_color::setup_render_color_callbacks`). A design can also carry a color that matches no
//! entry -- one picked from the Live Render toolbar, a variant that is no longer listed -- and
//! the combo then sits on "Custom...". Applying the panel must KEEP such a color rather than reset
//! the design to the material's own color (the combo is authoritative on Apply), so "Custom..."
//! means "leave the color as it is".
//!
//! The Slint combo hands an index back to Rust (`EditorModel.apply_design_material`). The list
//! the user sees is built here ([`shown_colors`]) and remembered for the Slint thread, so the
//! index of an Apply resolves against the SAME list the user actually saw, never against a list
//! rebuilt after the material changed.
//!
//! Everything but the remembered list is a pure function of its inputs so the rule is
//! unit-testable without a window.

use std::cell::RefCell;

use pleochroma::optics::{
    absorption::AbsorptionTensor,
    materials::{
        GemMaterial,
        variants::{ColorVariant, TensorRank, host_rank},
    },
};
use pleochroma_cut_core::{Design, MaterialLookup, MaterialSelection};
use pleochroma_editor::material::{
    ColorEntry, ColorEntryItem, color_entries_for_design, entry_for_selection,
    legacy_color_on_pleochroic_host, listed_variants, selection_with_entry,
};

use super::{
    material_lookup::{EditorMaterialLookup, traced_material_for},
    state::{
        body_color_custom_index, body_color_index_for, parse_design_material_form,
        with_body_color_choice,
    },
};

/// Whether `color` is a body colour that matches no preset (so the combo shows its
/// "Custom..." entry).
pub(super) fn is_custom(color: Option<[f32; 3]>) -> bool {
    color.is_some() && body_color_index_for(color) == body_color_custom_index()
}

/// The design's material as the host of its color: the material the design traces
/// (`traced_material_for`), WITHOUT any color override, so its rank and its own tensor are the
/// host's.
///
/// `None` when the design names no resolvable material, and for a design that names none at all:
/// its traced material is only a refractive-index guess, and a variant (or a Custom Color with a
/// rank) must never be stored for a guess.
pub(super) fn host_material(design: &Design, custom: &[GemMaterial]) -> Option<GemMaterial> {
    design.material.name.as_ref()?;
    let (name, unresolved) = traced_material_for(design, custom);
    if unresolved.is_some() {
        return None;
    }
    EditorMaterialLookup::new(custom).lookup(&name)
}

/// What the combo shows for the design on the bench ([`shown_colors`]).
pub(super) struct ShownColors {
    /// The rows, in combo order; Custom is last.
    pub(super) items: Vec<ColorEntryItem>,
    /// The row that shows the design's own color.
    pub(super) selected: usize,
    /// The design carries a legacy single color on a pleochroic material: the Stone tab shows the
    /// "Legacy Color: No Pleochroism" note with "Convert".
    pub(super) legacy_note: bool,
    /// The design holds a snapshot of a saved variant whose stored color has changed since: the
    /// Stone tab shows "Variant Changed: Update" next to the combo.
    pub(super) stale_variant: bool,
}

/// What a list was built from: it is rebuilt only when one of these changes, not on every panel
/// refresh (a refresh runs after every edit).
#[derive(Debug, Clone, PartialEq)]
struct ShownKey {
    host_name: String,
    host_absorption: AbsorptionTensor,
    host_rank: TensorRank,
    /// The legacy preset triple of the design (the "Legacy" row exists only for it).
    legacy_triple: Option<[f32; 3]>,
    /// The design stores the N-band form of the old L*C*h editor (no "Legacy" row then, even for
    /// a preset triple).
    legacy_bands: bool,
    /// The saved variants themselves (key, label and tensor), not only their number, so an
    /// in-place edit or rename of one rebuilds the list and its swatch.
    saved_variants: Vec<ColorVariant>,
}

/// Whether the design's color is a snapshot of a saved variant that no longer matches the stored
/// one.
///
/// The choice's key is a saved variant of `saved_variants` whose tensor, unit, reference width or
/// label differs (a rename is a change: "Update" refreshes the name the design shows). A variant
/// that was deleted, or a built-in or Custom Color, is never stale (there is nothing to update
/// from).
pub(super) fn variant_is_stale(
    material: &MaterialSelection,
    saved_variants: &[ColorVariant],
) -> bool {
    let Some(choice) = &material.color_variant_override else {
        return false;
    };
    saved_variants
        .iter()
        .find(|variant| variant.key == choice.key)
        .is_some_and(|stored| {
            stored.absorption != choice.absorption
                || stored.unit != choice.unit
                || stored.reference_path_mm.to_bits() != choice.reference_path_mm.to_bits()
                || stored.label != choice.label
        })
}

/// The list the combo was last given, and the variants behind its rows.
struct Shown {
    key: ShownKey,
    items: Vec<ColorEntryItem>,
    variants: Vec<ColorVariant>,
}

thread_local! {
    /// The combo's list as the Slint thread last saw it (see the module doc).
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// The isotropic stand-in host of a design whose material cannot be resolved (no name, no
/// matching refractive index): the plain list of the material's own color, the presets and Custom.
fn unresolved_host() -> GemMaterial {
    GemMaterial::new_custom("", 1.5, 0.0, 0.0, [0.0; 3])
}

/// The combo's rows for `design`, remembered for [`material_for_apply`].
///
/// `custom` is the catalogue of custom materials; `saved_variants` the cutter's saved color
/// variants (they are listed for their own material only).
pub(super) fn shown_colors(
    design: &Design,
    custom: &[GemMaterial],
    saved_variants: &[ColorVariant],
) -> ShownColors {
    let host = host_material(design, custom).unwrap_or_else(unresolved_host);
    let key = ShownKey {
        host_name: host.name.clone(),
        host_absorption: host.absorption.clone(),
        host_rank: host_rank(&host),
        legacy_triple: design.material.body_color_override,
        legacy_bands: design
            .material
            .body_color_bands_override
            .as_ref()
            .is_some_and(|bands| !bands.is_empty()),
        saved_variants: saved_variants.to_vec(),
    };
    let legacy_note = legacy_color_on_pleochroic_host(&design.material, &host);
    let stale_variant = variant_is_stale(&design.material, saved_variants);
    SHOWN.with_borrow_mut(|slot| {
        let shown = match slot.take() {
            Some(shown) if shown.key == key => shown,
            _ => Shown {
                items: color_entries_for_design(&host, saved_variants, &design.material),
                variants: listed_variants(&host, saved_variants),
                key,
            },
        };
        let selected = entry_for_selection(&design.material, &shown.items);
        let items = shown.items.clone();
        *slot = Some(shown);
        ShownColors {
            items,
            selected,
            legacy_note,
            stale_variant,
        }
    })
}

/// The entry at combo `index` of the list last shown, with the variants behind it; `None` before
/// any list was shown or for an index outside it.
fn shown_entry(index: i32) -> Option<(ColorEntry, Vec<ColorVariant>)> {
    let at = usize::try_from(index).ok()?;
    SHOWN.with_borrow(|slot| {
        let shown = slot.as_ref()?;
        Some((shown.items.get(at)?.entry.clone(), shown.variants.clone()))
    })
}

/// The material an Apply of the whole material row produces: the material/RI parse
/// ([`parse_design_material_form`]) with the color combo's row `color_index` applied on top
/// ([`selection_with_entry`]). A Custom row keeps the design's own color (a variant while the
/// material stays, the triple and the L*C*h editor's bands through a change of material name,
/// which the plain parse would drop); a variant or preset pick replaces it.
///
/// While the design has a color pattern (and the material stays) its color is the pattern's: the
/// combo is greyed beside it and its (stale) row is not applied, so the color and the pattern are
/// kept like a "Custom" row keeps them (an RI override never drops the pattern).
///
/// Before any list was shown the index falls back to the fixed-index contract
/// ([`with_body_color_choice`]).
pub(super) fn material_for_apply(
    combo_index: i32,
    ri_override_text: &str,
    options: &[String],
    current: &MaterialSelection,
    color_index: i32,
) -> Result<MaterialSelection, String> {
    let material = parse_design_material_form(combo_index, ri_override_text, options, current)?;
    // The parse keeps a pattern only while the material stays the same.
    if material.color_pattern_override.is_some() {
        return Ok(selection_with_entry(
            material,
            &ColorEntry::Custom,
            current,
            &[],
        ));
    }
    Ok(match shown_entry(color_index) {
        Some((entry, variants)) => selection_with_entry(material, &entry, current, &variants),
        None => with_body_color_choice(material, color_index, current),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::editor::state::EditorState;
    use pleochroma::optics::{absorption::AbsorptionBand, materials::AbsorptionUnit};
    use pleochroma_cut_core::material::{CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice};
    use pleochroma_editor::material::{
        body_color_for_apply, body_color_from_index, body_color_options,
    };

    /// A triple no preset has.
    const CUSTOM_RGB: [f32; 3] = [0.123, 0.456, 0.789];

    fn preset_rgb(index: i32) -> [f32; 3] {
        body_color_from_index(index).expect("a preset index")
    }

    /// A design with `material` set, on the editor's fresh state.
    fn design_with(material: MaterialSelection) -> Design {
        let mut design = EditorState::fresh().design.clone();
        design.material = material;
        design
    }

    fn sapphire() -> MaterialSelection {
        MaterialSelection {
            name: Some("Sapphire".to_owned()),
            ..MaterialSelection::none()
        }
    }

    /// The combo row of `entry` in the list just shown.
    fn row_of(shown: &ShownColors, entry: &ColorEntry) -> i32 {
        let at = shown
            .items
            .iter()
            .position(|item| item.entry == *entry)
            .expect("the row is listed");
        i32::try_from(at).expect("a small index")
    }

    #[test]
    fn no_color_and_preset_colors_are_not_custom() {
        assert!(!is_custom(None));
        for index in 1..body_color_custom_index() {
            assert!(!is_custom(Some(preset_rgb(index))), "preset {index}");
        }
    }

    #[test]
    fn a_non_preset_triple_is_custom() {
        assert!(is_custom(Some(CUSTOM_RGB)));
    }

    #[test]
    fn the_custom_entry_is_always_the_last_option() {
        let options = body_color_options();
        assert_eq!(
            usize::try_from(body_color_custom_index()).unwrap(),
            options.len() - 1
        );
        assert_eq!(
            body_color_index_for(Some(CUSTOM_RGB)),
            body_color_custom_index()
        );
    }

    #[test]
    fn apply_keeps_a_custom_triple_when_custom_is_selected() {
        assert_eq!(
            body_color_for_apply(body_color_custom_index(), Some(CUSTOM_RGB)),
            Some(CUSTOM_RGB)
        );
    }

    #[test]
    fn apply_resolves_presets_and_the_default_as_before() {
        assert_eq!(body_color_for_apply(0, Some(CUSTOM_RGB)), None);
        assert_eq!(
            body_color_for_apply(2, Some(CUSTOM_RGB)),
            Some(preset_rgb(2)),
            "choosing a preset replaces the custom triple"
        );
        assert_eq!(body_color_for_apply(2, None), Some(preset_rgb(2)));
    }

    #[test]
    fn apply_design_material_keeps_a_custom_triple() {
        let current = MaterialSelection::default().with_body_color(Some(CUSTOM_RGB));
        let options = vec!["(none)".to_owned(), "Diamond".to_owned()];
        // The material row is left on "(none)", no RI text, and the colour combo sits
        // on its Custom entry: the triple stays.
        let applied = material_for_apply(0, "", &options, &current, body_color_custom_index())
            .expect("the form parses");
        assert_eq!(applied.body_color_override, Some(CUSTOM_RGB));
        // The plain material parse drops the colour when the species changes ...
        let parsed = parse_design_material_form(1, "", &options, &current).expect("parses");
        assert_eq!(parsed.name.as_deref(), Some("Diamond"));
        assert_eq!(parsed.body_color_override, None);
        // ... but the combo is authoritative, so a Custom selection keeps the triple
        // through a species change too.
        let switched = material_for_apply(1, "", &options, &current, body_color_custom_index())
            .expect("the form parses");
        assert_eq!(switched.name.as_deref(), Some("Diamond"));
        assert_eq!(switched.body_color_override, Some(CUSTOM_RGB));
    }

    #[test]
    fn apply_keeps_the_lch_bands_for_custom_and_drops_them_for_a_preset() {
        let rows = vec![[460.0f32, 45.0, 0.25]];
        let current = MaterialSelection::default().with_body_color_bands(
            Some(CUSTOM_RGB),
            Some(rows.clone()),
            Some(2.0),
        );
        let options = vec!["(none)".to_owned()];
        let kept = material_for_apply(0, "", &options, &current, body_color_custom_index())
            .expect("parses");
        assert_eq!(kept.body_color_bands_override, Some(rows));
        assert_eq!(kept.absorption_path_scale_override, Some(2.0));
        let preset = material_for_apply(0, "", &options, &current, 4).expect("parses");
        assert_eq!(preset.body_color_bands_override, None);
        assert_eq!(preset.absorption_path_scale_override, None);
        let cleared = material_for_apply(0, "", &options, &current, 0).expect("parses");
        assert_eq!(cleared.body_color_bands_override, None);
    }

    #[test]
    fn apply_design_material_can_still_clear_or_replace_the_color() {
        let current = MaterialSelection::default().with_body_color(Some(CUSTOM_RGB));
        let options = vec!["(none)".to_owned()];
        let cleared = material_for_apply(0, "", &options, &current, 0).expect("parses");
        assert_eq!(cleared.body_color_override, None);
        let preset = material_for_apply(0, "", &options, &current, 4).expect("parses");
        assert_eq!(preset.body_color_override, Some(preset_rgb(4)));
    }

    /// The combo of a Sapphire lists its variants and no generic preset; picking a variant row
    /// stores that variant, keeps the stone Sapphire, and the combo then shows that row.
    #[test]
    fn picking_a_variant_row_stores_it_and_the_combo_then_shows_it() {
        let design = design_with(sapphire());
        let shown = shown_colors(&design, &[], &[]);
        assert_eq!(
            shown.items[shown.selected].entry,
            ColorEntry::MaterialDefault
        );
        assert!(!shown.legacy_note);
        assert!(
            !shown
                .items
                .iter()
                .any(|item| matches!(item.entry, ColorEntry::Preset(_)))
        );
        let first_variant = shown
            .items
            .iter()
            .find_map(|item| match &item.entry {
                ColorEntry::Variant { key } => Some(key.clone()),
                _ => None,
            })
            .expect("Sapphire ships variants");
        let row = row_of(
            &shown,
            &ColorEntry::Variant {
                key: first_variant.clone(),
            },
        );

        let options = vec!["(none)".to_owned(), "Sapphire".to_owned()];
        let applied = material_for_apply(1, "", &options, &design.material, row).expect("parses");
        assert_eq!(applied.name.as_deref(), Some("Sapphire"));
        assert_eq!(
            applied
                .color_variant_override
                .as_ref()
                .map(|c| c.key.as_str()),
            Some(first_variant.as_str())
        );
        assert_eq!(applied.body_color_override, None);

        let reshown = shown_colors(&design_with(applied), &[], &[]);
        assert_eq!(
            reshown.items[reshown.selected].entry,
            ColorEntry::Variant { key: first_variant }
        );
    }

    /// An Apply of the material row on a design with a Custom Color keeps it, and the combo sits
    /// on Custom (never on "Material Default").
    #[test]
    fn a_custom_color_survives_an_apply_of_the_material_row() {
        let choice = VariantChoice {
            key: CUSTOM_VARIANT_KEY.to_owned(),
            label: CUSTOM_VARIANT_LABEL.to_owned(),
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::energy(540.0, 3000.0, 0.6)],
                vec![AbsorptionBand::energy(450.0, 3000.0, 0.3)],
            ),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 5.0,
        };
        let current = sapphire().with_color_variant(Some(choice.clone()));
        let design = design_with(current.clone());
        let shown = shown_colors(&design, &[], &[]);
        assert_eq!(shown.items[shown.selected].entry, ColorEntry::Custom);
        let options = vec![
            "(none)".to_owned(),
            "Sapphire".to_owned(),
            "Diamond".to_owned(),
        ];
        let kept = material_for_apply(
            1,
            "",
            &options,
            &current,
            i32::try_from(shown.selected).unwrap(),
        )
        .expect("parses");
        assert_eq!(kept.color_variant_override, Some(choice));
        // Switching the species with the combo on Custom: the Sapphire color has no business on
        // Diamond, so the variant is dropped.
        let switched = material_for_apply(
            2,
            "",
            &options,
            &current,
            i32::try_from(shown.selected).unwrap(),
        )
        .expect("parses");
        assert_eq!(switched.name.as_deref(), Some("Diamond"));
        assert_eq!(switched.color_variant_override, None);
    }

    /// An old file with a preset triple on Sapphire keeps rendering as saved: the combo shows it
    /// as the Legacy row, the note is on, and an Apply keeps the triple.
    #[test]
    fn a_legacy_preset_on_a_pleochroic_stone_shows_the_legacy_row() {
        // Combo index 2 is the second preset (index 0 is the material's own color).
        let current = sapphire().with_body_color(Some(preset_rgb(2)));
        let design = design_with(current.clone());
        let shown = shown_colors(&design, &[], &[]);
        assert!(shown.legacy_note);
        assert_eq!(
            shown.items[shown.selected].label,
            "Legacy: Blue (No Pleochroism)"
        );
        let options = vec!["(none)".to_owned(), "Sapphire".to_owned()];
        let kept = material_for_apply(
            1,
            "",
            &options,
            &current,
            i32::try_from(shown.selected).unwrap(),
        )
        .expect("parses");
        assert_eq!(kept.body_color_override, Some(preset_rgb(2)));
    }

    /// A saved Sapphire variant with one band per ray of the given peaks.
    fn saved(key: &str, label: &str, o_peak: f32, e_peak: f32) -> ColorVariant {
        ColorVariant {
            key: key.to_owned(),
            label: label.to_owned(),
            material: "Sapphire".to_owned(),
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::energy(540.0, 3000.0, o_peak)],
                vec![AbsorptionBand::energy(450.0, 3000.0, e_peak)],
            ),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
            provenance: pleochroma::optics::materials::variants::VariantProvenance::User,
        }
    }

    /// A design whose snapshot is exactly `stored` is current; once the stored variant is edited
    /// the snapshot is stale; a deleted variant, a built-in key and no color are never stale.
    #[test]
    fn a_snapshot_of_an_edited_saved_variant_is_stale() {
        let stored = saved("user.1", "Mine", 0.6, 0.3);
        let snapshot = VariantChoice::from_variant(&stored);
        let design = sapphire().with_color_variant(Some(snapshot));
        assert!(!variant_is_stale(&design, std::slice::from_ref(&stored)));
        let edited = saved("user.1", "Mine", 0.9, 0.3);
        assert!(variant_is_stale(&design, &[edited]));
        // The variant is gone from the vault: nothing to update from.
        assert!(!variant_is_stale(&design, &[]));
        assert!(!variant_is_stale(&sapphire(), &[stored]));
        // Another key is not this design's variant.
        let other = saved("user.2", "Other", 0.9, 0.9);
        assert!(!variant_is_stale(&design, &[other]));
    }

    /// Review finding 8: a rename of the saved variant leaves the tensor alone but is still a
    /// change the design should be offered ("Update" refreshes the name it shows).
    #[test]
    fn a_renamed_saved_variant_makes_the_snapshot_stale() {
        let stored = saved("user.1", "Mine", 0.6, 0.3);
        let design = sapphire().with_color_variant(Some(VariantChoice::from_variant(&stored)));
        let mut renamed = stored;
        renamed.label = "Deeper".to_owned();
        assert!(variant_is_stale(&design, std::slice::from_ref(&renamed)));
        // "Update" copies the new name in: the snapshot is current afterwards.
        let updated = design.with_color_variant(Some(VariantChoice::from_variant(&renamed)));
        assert!(!variant_is_stale(&updated, &[renamed]));
    }

    /// Review finding 2 (scenario B): a design with a color pattern keeps its color and pattern
    /// through an Apply of the material row, whatever row the greyed combo holds.
    #[test]
    fn a_material_row_apply_keeps_the_pattern_and_the_variant() {
        use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
        let first_variant = {
            let shown = shown_colors(&design_with(sapphire()), &[], &[]);
            shown
                .items
                .iter()
                .find_map(|item| match &item.entry {
                    ColorEntry::Variant { key } => Some(key.clone()),
                    _ => None,
                })
                .expect("Sapphire ships variants")
        };
        let options = vec!["(none)".to_owned(), "Sapphire".to_owned()];
        let picked = material_for_apply(
            1,
            "",
            &options,
            &sapphire(),
            row_of(
                &shown_colors(&design_with(sapphire()), &[], &[]),
                &ColorEntry::Variant { key: first_variant },
            ),
        )
        .expect("parses");
        let pattern = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
        let current = picked.with_color_pattern(Some(pattern.clone()));
        let design = design_with(current.clone());
        let shown = shown_colors(&design, &[], &[]);
        // The combo holds ANY row (here Material Default, row 0): the pattern design ignores it.
        for row in 0..shown.items.len() {
            let applied = material_for_apply(
                1,
                "1.77",
                &options,
                &current,
                i32::try_from(row).expect("a small index"),
            )
            .expect("parses");
            assert_eq!(
                applied.color_pattern_override,
                Some(pattern.clone()),
                "{row}"
            );
            assert_eq!(
                applied.color_variant_override,
                current.color_variant_override
            );
            assert_eq!(applied.refractive_index_override, Some(1.77));
        }
    }

    /// Review web finding 5: a design with no material name lists no variants (its traced
    /// material is only a refractive-index guess) and has no host for a Custom Color.
    #[test]
    fn a_nameless_design_has_no_host_and_lists_no_variants() {
        let mut design = design_with(MaterialSelection::none());
        // A refractive index that matches a pleochroic built-in.
        design.meta.refractive_index = 1.77;
        assert!(host_material(&design, &[]).is_none());
        let shown = shown_colors(&design, &[], &[]);
        assert!(
            shown
                .items
                .iter()
                .all(|item| !matches!(item.entry, ColorEntry::Variant { .. })),
            "no variant row for a nameless design"
        );
    }

    /// An in-place edit of a saved variant (same key, same count) rebuilds the list, so the combo
    /// row and its swatch show the new color, and the design is then flagged stale.
    #[test]
    fn editing_a_saved_variant_in_place_refreshes_the_list() {
        let before = saved("user.1", "Mine", 0.6, 0.3);
        let design =
            design_with(sapphire().with_color_variant(Some(VariantChoice::from_variant(&before))));
        let first = shown_colors(&design, &[], std::slice::from_ref(&before));
        let at = first
            .items
            .iter()
            .position(|item| item.label == "Mine")
            .expect("listed");
        assert!(!first.stale_variant);
        let after = saved("user.1", "Mine", 1.8, 0.1);
        let second = shown_colors(&design, &[], &[after]);
        let again = second
            .items
            .iter()
            .position(|item| item.label == "Mine")
            .expect("still listed");
        assert_eq!(at, again);
        assert_ne!(
            first.items[at].swatch_hex, second.items[again].swatch_hex,
            "the swatch follows the edited tensor"
        );
        assert!(second.stale_variant);
        // A rename in place refreshes the label too.
        let renamed = saved("user.1", "Deeper", 1.8, 0.1);
        let third = shown_colors(&design, &[], &[renamed]);
        assert!(third.items.iter().any(|item| item.label == "Deeper"));
    }

    /// A design on an isotropic material keeps the old contract: the presets are listed.
    #[test]
    fn an_isotropic_stone_lists_the_presets() {
        let design = design_with(MaterialSelection {
            name: Some("Diamond".to_owned()),
            ..MaterialSelection::none()
        });
        let shown = shown_colors(&design, &[], &[]);
        let presets = shown
            .items
            .iter()
            .filter(|item| matches!(item.entry, ColorEntry::Preset(_)))
            .count();
        assert_eq!(presets, 9);
        assert!(!shown.legacy_note);
    }

    #[test]
    fn the_listed_material_is_the_hosts_own_with_no_color_override() {
        let design = design_with(sapphire().with_color_variant(Some(VariantChoice {
            key: "sapphire.test".to_owned(),
            label: "Test".to_owned(),
            absorption: AbsorptionTensor::uniaxial(Vec::new(), Vec::new()),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
        })));
        let host = host_material(&design, &[]).expect("Sapphire resolves");
        assert_eq!(host.absorption, GemMaterial::sapphire().absorption);
        let unnamed = design_with(MaterialSelection::none());
        // No name and the fresh design's refractive index may or may not match a preset; either
        // way the stand-in list is the plain one.
        let shown = shown_colors(&unnamed, &[], &[]);
        assert_eq!(
            shown.items.last().map(|item| &item.entry),
            Some(&ColorEntry::Custom)
        );
    }
}
