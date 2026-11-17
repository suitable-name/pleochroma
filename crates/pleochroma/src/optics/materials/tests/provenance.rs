//! Tests of the optics provenance records of the built-in materials.

use crate::optics::materials::{GemMaterial, optics_provenance};

/// Every built-in has a record whose two texts say something, and no record exists for a name
/// that is not a built-in.
#[test]
fn every_builtin_has_a_nonempty_optics_provenance_record() {
    let materials = GemMaterial::all_materials();
    assert_eq!(materials.len(), 39, "the built-in table has 39 materials");
    for material in &materials {
        let record = optics_provenance(&material.name)
            .unwrap_or_else(|| panic!("{} has no optics provenance record", material.name));
        assert!(
            !record.dispersion.trim().is_empty(),
            "{}: empty dispersion source",
            material.name
        );
        assert!(
            !record.absorption.trim().is_empty(),
            "{}: empty absorption source",
            material.name
        );
    }
    assert_eq!(optics_provenance("Unobtainium"), None);
}

/// The texts carry no fluorescence wording and no UV shorthand of their own; a published title
/// is quoted as printed ("Application of UV-Vis-NIR Spectroscopy to Gemology").
#[test]
fn provenance_texts_carry_no_excluded_words() {
    const PUBLISHED_TITLE: &str = "Application of UV-Vis-NIR Spectroscopy to Gemology";
    for material in GemMaterial::all_materials() {
        let record = optics_provenance(&material.name).expect("a built-in has a record");
        for text in [record.dispersion, record.absorption] {
            let own = text.replace(PUBLISHED_TITLE, "");
            let lower = own.to_lowercase();
            assert!(
                !lower.contains("fluoresc") && !lower.contains("ultraviolet"),
                "{}: {text}",
                material.name
            );
            assert!(!own.contains("UV"), "{}: {text}", material.name);
        }
    }
}

/// A few records are pinned: the name match ignores letter case, a primary-source curve names
/// its paper, the colorless materials are not estimated, and a spectrum with an unmeasured ray
/// is flagged as estimated.
#[test]
fn a_few_records_are_pinned() {
    let diamond = optics_provenance("diamond").expect("Diamond has a record");
    assert!(diamond.dispersion.contains("Peter"));
    assert!(!diamond.absorption_estimated);

    let sapphire = optics_provenance("Sapphire").expect("Sapphire has a record");
    assert!(sapphire.dispersion.contains("Malitson"));
    assert!(sapphire.absorption.contains("Dubinsky"));

    let alexandrite = optics_provenance("Alexandrite").expect("Alexandrite has a record");
    assert!(alexandrite.dispersion.contains("Walling"));
    assert!(alexandrite.absorption_estimated);

    // The four quartz-host entries cite the same dispersion paper.
    let quartz = optics_provenance("Quartz").expect("Quartz has a record");
    for name in ["Amethyst", "Citrine"] {
        let record = optics_provenance(name).expect("a quartz-host entry has a record");
        assert_eq!(record.dispersion, quartz.dispersion, "{name}");
    }
    assert!(quartz.dispersion.contains("Ghosh"));
}
