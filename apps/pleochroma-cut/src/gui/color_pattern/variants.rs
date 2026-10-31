//! The color variants a zone color can be: those of the stone's material that fit it.

use std::sync::{Mutex, PoisonError};

use pleochroma::optics::materials::{
    GemMaterial,
    variants::{ColorVariant, built_in_variants},
};
use pleochroma_cut_core::material::pattern::{EXACT_COLOR_KEY, PatternColor};
use pleochroma_vault::db::sqlite::Database;
use tracing::warn;

use crate::gui::optics::color_variant_store::{estimated_note, user_color_variants};

/// The suffix the list adds to the label of a variant the pattern holds but the material no longer
/// lists (a saved variant that was deleted, or a pattern opened on another material).
const KEPT_SUFFIX: &str = "(Kept In Pattern)";

/// The color variants of `host` that can be a zone color of a stone made of it: the built-in
/// variants of its material (the Material Default first), then the cutter's saved ones, each only
/// if it passes `ColorVariant::validate_for` for `host` (the rank rule of the design color).
///
/// A custom material has no built-in variants; its list is the cutter's saved ones. `db` is the
/// vault the saved ones are read from; `None` (no vault, as in a test) lists the built-in ones only.
pub fn zone_variants(host: &GemMaterial, db: Option<&Mutex<Database>>) -> Vec<ColorVariant> {
    let mut variants = built_in_variants(&host.name);
    variants.extend(db.map_or_else(Vec::new, |db| user_variants(db, &host.name)));
    variants.retain(|variant| variant.validate_for(host).is_ok());
    variants
}

/// The cutter's saved variants of the material called `material_name`, read from `db`. A vault
/// that cannot be read lists none (logged): the zone picker still offers the built-in ones.
fn user_variants(db: &Mutex<Database>, material_name: &str) -> Vec<ColorVariant> {
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    user_color_variants(&guard, material_name).unwrap_or_else(|error| {
        warn!("Saved colors of '{material_name}' could not be read: {error:#}");
        Vec::new()
    })
}

/// The sentence under the Variant menu for an estimated built-in variant (`"Estimated: <first
/// source line>"`), or `None` for any other variant.
pub(super) fn estimated_line(variant: &ColorVariant) -> Option<String> {
    estimated_note(&variant.provenance)
}

/// The labels of the variant menu for a zone holding `color`, and the entry the zone has (`-1`
/// when it holds no variant). A variant the pattern holds that `variants` does not list is added
/// at the end, so the menu never shows a variant the zone does not have.
pub(super) fn variant_menu(variants: &[ColorVariant], color: &PatternColor) -> (Vec<String>, i32) {
    let mut labels: Vec<String> = variants
        .iter()
        .map(|variant| {
            if estimated_line(variant).is_some() {
                format!("{} (Estimated)", variant.label)
            } else {
                variant.label.clone()
            }
        })
        .collect();
    let at = match color {
        PatternColor::Variant { key, label, .. } => {
            let found = variants.iter().position(|variant| variant.key == *key);
            Some(found.unwrap_or_else(|| {
                // The stone's own exact color needs no note: it is what the stone looks like.
                if key == EXACT_COLOR_KEY {
                    labels.push(label.clone());
                } else {
                    labels.push(format!("{label} {KEPT_SUFFIX}"));
                }
                labels.len() - 1
            }))
        }
        _ => None,
    };
    let index = at.and_then(|at| i32::try_from(at).ok()).unwrap_or(-1);
    (labels, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::{
        absorption::AbsorptionTensor,
        materials::{
            AbsorptionUnit,
            variants::{VariantProvenance, host_rank},
        },
    };

    fn variant(key: &str, label: &str) -> ColorVariant {
        ColorVariant {
            key: key.to_owned(),
            label: label.to_owned(),
            material: "Sapphire".to_owned(),
            absorption: AbsorptionTensor::uniaxial(Vec::new(), Vec::new()),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
            provenance: VariantProvenance::User,
        }
    }

    #[test]
    fn a_sapphire_lists_its_default_first_and_only_variants_that_fit_it() {
        let host = GemMaterial::sapphire();
        let list = zone_variants(&host, None);
        assert_eq!(list[0].key, "sapphire.default");
        assert!(list.iter().any(|variant| variant.key == "sapphire.yellow"));
        for variant in &list {
            assert_eq!(variant.validate_for(&host), Ok(()), "{}", variant.key);
        }
        assert_eq!(
            host_rank(&host),
            pleochroma::optics::materials::variants::TensorRank::Uniaxial
        );
    }

    #[test]
    fn a_custom_material_has_no_built_in_variants() {
        let mut host = GemMaterial::sapphire();
        host.name = "My Own Corundum".to_owned();
        assert!(zone_variants(&host, None).is_empty());
    }

    #[test]
    fn the_cutters_saved_variants_follow_the_built_in_ones() {
        use crate::gui::optics::color_variant_store::save_user_color_variant;
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory vault"));
        let host = GemMaterial::sapphire();
        let mut mine = variant("", "Mine");
        mine.absorption = AbsorptionTensor::uniaxial(
            vec![pleochroma::optics::absorption::AbsorptionBand::energy(
                540.0, 3000.0, 0.6,
            )],
            vec![pleochroma::optics::absorption::AbsorptionBand::energy(
                450.0, 3000.0, 0.3,
            )],
        );
        let key = save_user_color_variant(&db.lock().unwrap(), &mine).expect("saved");
        let list = zone_variants(&host, Some(&db));
        assert_eq!(list.last().map(|v| v.key.as_str()), Some(key.as_str()));
        assert_eq!(list.last().map(|v| v.label.as_str()), Some("Mine"));
        assert!(list.len() > zone_variants(&host, None).len());
    }

    #[test]
    fn the_menu_marks_an_estimated_built_in_variant() {
        let mut estimated = variant("sapphire.x", "Pink");
        estimated.provenance = VariantProvenance::BuiltIn {
            sources: vec!["Cr3+ bands; estimated".to_owned()],
            fit_record: None,
            estimated: true,
        };
        let none = PatternColor::Single(
            pleochroma_cut_core::material::pattern::ZoneColor::unsolved([50.0, 20.0, 80.0]),
        );
        let (labels, _) = variant_menu(&[estimated, variant("user.a", "Mine")], &none);
        assert_eq!(labels, ["Pink (Estimated)", "Mine"]);
    }

    #[test]
    fn only_an_estimated_built_in_has_an_estimated_line() {
        let mut estimated = variant("sapphire.x", "X");
        estimated.provenance = VariantProvenance::BuiltIn {
            sources: vec!["Fe3+ bands; estimated".to_owned()],
            fit_record: None,
            estimated: true,
        };
        assert_eq!(
            estimated_line(&estimated).as_deref(),
            Some("Estimated: Fe3+ bands; estimated")
        );
        assert_eq!(estimated_line(&variant("user.a", "A")), None);
        estimated.provenance = VariantProvenance::BuiltIn {
            sources: vec!["Rossman".to_owned()],
            fit_record: None,
            estimated: false,
        };
        assert_eq!(estimated_line(&estimated), None);
    }

    #[test]
    fn the_menu_marks_the_variant_the_zone_has_and_keeps_one_the_list_lacks() {
        let list = [variant("a.one", "One"), variant("a.two", "Two")];
        let two = PatternColor::from_variant(&list[1]);
        assert_eq!(
            variant_menu(&list, &two),
            (vec!["One".to_owned(), "Two".to_owned()], 1)
        );
        let custom = PatternColor::Single(
            pleochroma_cut_core::material::pattern::ZoneColor::unsolved([50.0, 20.0, 80.0]),
        );
        assert_eq!(variant_menu(&list, &custom).1, -1);
        let gone = PatternColor::from_variant(&variant("user.gone", "Mine"));
        let (labels, index) = variant_menu(&list, &gone);
        assert_eq!(labels.len(), 3);
        assert_eq!(index, 2);
        assert_eq!(labels[2], "Mine (Kept In Pattern)");
    }
}
