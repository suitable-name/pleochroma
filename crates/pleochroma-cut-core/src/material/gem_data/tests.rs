//! Tests of the built-in gem reference table: coverage, ranges, sources, pinned summaries, the
//! banned-words rule and the exact set of empty fields.

use super::{
    Cited, GemBasis, GemData, GemSource, Toughness, built_in_gem_data, cleavage_summary,
    fracture_summary, mohs_label, parting_summary, table::TABLE,
};
use pleochroma::optics::materials::GemMaterial;

/// The data of a material that must be in the table.
fn data(name: &str) -> &'static GemData {
    built_in_gem_data(name).unwrap_or_else(|| panic!("no gem data for {name}"))
}

/// The sources of every stored value of an entry, with a label for failure messages.
fn all_source_lists(name: &str, entry: &GemData) -> Vec<(String, &'static [GemSource])> {
    let mut lists: Vec<(String, &'static [GemSource])> =
        vec![(format!("{name} mohs"), entry.mohs.sources)];
    if let Some(knoop) = &entry.knoop {
        lists.push((format!("{name} knoop"), knoop.sources));
    }
    if let Some(toughness) = &entry.toughness {
        lists.push((format!("{name} toughness"), toughness.sources));
    }
    if let Some(cleavage) = &entry.cleavage {
        lists.push((format!("{name} cleavage"), cleavage.sources));
    }
    if let Some(parting) = &entry.parting {
        lists.push((format!("{name} parting"), parting.sources));
    }
    if let Some(fracture) = &entry.fracture {
        lists.push((format!("{name} fracture"), fracture.sources));
    }
    for (index, (topic, note)) in entry.care.iter().enumerate() {
        lists.push((
            format!("{name} care {index} {}", topic.label()),
            note.sources,
        ));
    }
    for (index, note) in entry.notes.iter().enumerate() {
        lists.push((format!("{name} note {index}"), note.sources));
    }
    lists
}

/// T-D1: every built-in material has an entry, and no other name has one.
#[test]
fn every_built_in_has_an_entry_and_no_other_name_does() {
    let built_ins: Vec<String> = GemMaterial::all_materials()
        .into_iter()
        .map(|material| material.name)
        .collect();
    assert_eq!(built_ins.len(), 40);
    assert_eq!(TABLE.len(), built_ins.len());
    for name in &built_ins {
        assert!(
            built_in_gem_data(name).is_some(),
            "{name} has no gem data entry"
        );
    }
    for (name, _) in TABLE {
        assert!(
            built_ins.iter().any(|built_in| built_in.as_str() == *name),
            "{name} is not a built-in material name"
        );
    }
    for (index, (name, _)) in TABLE.iter().enumerate() {
        assert!(
            TABLE[..index]
                .iter()
                .all(|(earlier, _)| !earlier.eq_ignore_ascii_case(name)),
            "{name} appears twice"
        );
    }
    assert!(
        built_in_gem_data("diamond").is_some(),
        "lookup ignores ASCII case"
    );
    assert!(built_in_gem_data("Garnet").is_none());
    assert!(built_in_gem_data("My Blue Sapphire").is_none());
}

/// T-D2: Mohs is `1 <= low <= high <= 10` and toughness is `low <= high`.
#[test]
fn ranges_are_ordered() {
    for (name, entry) in TABLE {
        let (low, high) = entry.mohs.value;
        assert!(
            1.0 <= low && low <= high && high <= 10.0,
            "{name}: Mohs {low}-{high}"
        );
        if let Some(toughness) = &entry.toughness {
            assert!(
                toughness.value.low <= toughness.value.high,
                "{name}: toughness range is reversed"
            );
        }
    }
}

/// T-D3: every stored value has at least one source, every source renders a citation, and every
/// address starts with `http`.
#[test]
fn every_value_has_a_rendering_source() {
    for (name, entry) in TABLE {
        for (label, sources) in all_source_lists(name, entry) {
            assert!(!sources.is_empty(), "{label} has no source");
            for source in sources {
                assert!(
                    !source.citation().trim().is_empty(),
                    "{label}: empty citation for {source:?}"
                );
                if let Some(url) = source.url() {
                    assert!(
                        url.starts_with("https://") || url.starts_with("http://"),
                        "{label}: address {url} is not http(s)"
                    );
                }
            }
        }
    }
}

/// T-D4: the summaries of a few pinned materials.
#[test]
fn pinned_summaries() {
    let topaz = data("Topaz");
    let cleavage = topaz.cleavage.as_ref().map(|c| cleavage_summary(c.value));
    assert_eq!(cleavage.as_deref(), Some("Perfect basal {001}"));

    assert_eq!(mohs_label(data("Emerald").mohs.value), "7.5\u{2013}8");
    assert_eq!(mohs_label(data("Sapphire").mohs.value), "9");
    assert_eq!(mohs_label(data("Synthetic Moissanite").mohs.value), "9.25");

    let emerald = data("Emerald").toughness.as_ref().map(|t| t.value.label());
    assert_eq!(emerald.as_deref(), Some("Fair to good"));
    let diamond = data("Diamond").toughness.as_ref().map(|t| t.value.label());
    assert_eq!(
        diamond.as_deref(),
        Some("Good to exceptional (by direction)")
    );
    let tanzanite = data("Tanzanite")
        .toughness
        .as_ref()
        .map(|t| t.value.label());
    assert_eq!(tanzanite.as_deref(), Some("Poor to fair"));
    let sapphire = data("Sapphire").toughness.as_ref().map(|t| t.value.label());
    assert_eq!(sapphire.as_deref(), Some("Excellent"));

    let alexandrite = data("Alexandrite")
        .cleavage
        .as_ref()
        .map(|c| cleavage_summary(c.value));
    assert_eq!(
        alexandrite.as_deref(),
        Some("Distinct {110}, imperfect {010}, poor {001}")
    );
    let tourmaline = data("Tourmaline")
        .cleavage
        .as_ref()
        .map(|c| cleavage_summary(c.value));
    assert_eq!(tourmaline.as_deref(), Some("Very poor {11-20}, {10-11}"));
    let sapphire_cleavage = data("Sapphire")
        .cleavage
        .as_ref()
        .map(|c| cleavage_summary(c.value));
    assert_eq!(sapphire_cleavage.as_deref(), Some("None"));
    let diamond_cleavage = data("Diamond").cleavage.as_ref().map(|c| c.value);
    assert_eq!(
        diamond_cleavage.map(cleavage_summary).as_deref(),
        Some("Perfect octahedral {111}")
    );
    assert_eq!(
        diamond_cleavage.and_then(|c| c.first().and_then(|d| d.directions)),
        Some(4)
    );

    let sapphire_parting = data("Sapphire")
        .parting
        .as_ref()
        .map(|p| parting_summary(p.value));
    assert_eq!(
        sapphire_parting.as_deref(),
        Some("Basal {0001}, rhombohedral {10-11}")
    );

    let spinel = data("Spinel")
        .fracture
        .as_ref()
        .map(|f| fracture_summary(f.value));
    assert_eq!(spinel.as_deref(), Some("Conchoidal, uneven to splintery"));
    let sapphire_fracture = data("Sapphire")
        .fracture
        .as_ref()
        .map(|f| fracture_summary(f.value));
    assert_eq!(sapphire_fracture.as_deref(), Some("Uneven to conchoidal"));
    let quartz = data("Quartz")
        .fracture
        .as_ref()
        .map(|f| fracture_summary(f.value));
    assert_eq!(quartz.as_deref(), Some("Conchoidal"));

    assert_eq!(data("Glass (N-BK7)").knoop.map(|k| k.value), Some(610));
    assert_eq!(data("Glass (F2)").knoop.map(|k| k.value), Some(420));
    assert!(matches!(
        data("Color-Change Garnet (Pyrope-Spessartine)").basis,
        GemBasis::Inferred(_)
    ));
    assert_eq!(Toughness::VeryPoor.rank(), 0);
    assert_eq!(Toughness::Exceptional.rank(), 5);
}

/// Every text the table stores or renders: species, notes, care, basis and the citations.
fn all_text(name: &str, entry: &GemData) -> Vec<String> {
    let mut texts = vec![name.to_owned(), entry.species.to_owned()];
    match entry.basis {
        GemBasis::Species => {}
        GemBasis::Inferred(note) | GemBasis::Derived(note) => texts.push(note.to_owned()),
    }
    for note in entry.notes {
        texts.push(note.text.to_owned());
    }
    for (topic, note) in entry.care {
        texts.push(topic.label().to_owned());
        texts.push(note.value.to_owned());
    }
    if let Some(cleavage) = &entry.cleavage {
        texts.push(cleavage_summary(cleavage.value));
    }
    if let Some(parting) = &entry.parting {
        texts.push(parting_summary(parting.value));
    }
    if let Some(fracture) = &entry.fracture {
        texts.push(fracture_summary(fracture.value));
    }
    if let Some(toughness) = &entry.toughness {
        texts.push(toughness.value.label());
    }
    for (_, sources) in all_source_lists(name, entry) {
        for source in sources {
            texts.push(source.citation());
            texts.extend(source.url());
        }
    }
    texts
}

/// T-D5: no stored or rendered text mentions fluorescence, ultraviolet light or phosphorescence.
#[test]
fn no_banned_words_in_the_table() {
    const BANNED: [&str; 4] = ["fluoresc", "uv", "ultraviolet", "phosphoresc"];
    for (name, entry) in TABLE {
        for text in all_text(name, entry) {
            let lower = text.to_lowercase();
            for word in BANNED {
                assert!(
                    !lower.contains(word),
                    "{name}: \"{text}\" contains \"{word}\""
                );
            }
        }
    }
}

/// T-D6: the empty fields are exactly those of the plan, so a value cannot appear or vanish
/// silently.
#[test]
fn the_empty_fields_are_exactly_the_planned_ones() {
    let mut empty: Vec<(String, &str)> = Vec::new();
    for (name, entry) in TABLE {
        if entry.toughness.is_none() {
            empty.push(((*name).to_owned(), "toughness"));
        }
        if entry.cleavage.is_none() {
            empty.push(((*name).to_owned(), "cleavage"));
        }
        if entry.fracture.is_none() {
            empty.push(((*name).to_owned(), "fracture"));
        }
    }
    empty.sort();
    let mut expected: Vec<(String, &str)> = vec![
        ("Glass (N-BK7)".to_owned(), "toughness"),
        ("Glass (F2)".to_owned(), "toughness"),
        ("Benitoite".to_owned(), "toughness"),
        ("Titanite (Sphene)".to_owned(), "toughness"),
        ("Nanosital".to_owned(), "toughness"),
        ("Nanosital".to_owned(), "cleavage"),
        ("GGG".to_owned(), "cleavage"),
        ("GGG".to_owned(), "fracture"),
    ];
    expected.sort();
    assert_eq!(empty, expected);
}

/// The citation and address texts of one source of each kind.
#[test]
fn source_citations_and_addresses() {
    assert_eq!(
        GemSource::GiaCare("topaz").citation(),
        "GIA, Topaz care guide"
    );
    assert_eq!(
        GemSource::GiaCare("topaz").url().as_deref(),
        Some("https://www.gia.edu/topaz-care-cleaning")
    );
    assert_eq!(
        GemSource::HandbookOfMineralogy("chrysoberyl").citation(),
        "Handbook of Mineralogy, Chrysoberyl"
    );
    assert_eq!(
        GemSource::HandbookOfMineralogy("Spinel").url().as_deref(),
        Some("https://www.handbookofmineralogy.org/pdfs/Spinel.pdf")
    );
    assert_eq!(GemSource::GiaGemReferenceGuide.url(), None);
    assert_eq!(
        GemSource::Wikipedia("Cubic_zirconia").citation(),
        "Wikipedia, Cubic zirconia"
    );
    assert_eq!(
        GemSource::GemsGemology2026.url().as_deref(),
        Some("https://doi.org/10.5741/GEMS.62.2.164")
    );
    assert_eq!(
        GemSource::RussianJeweller2016.citation(),
        "Russian Jeweller, 13 July 2016 (manufacturer's text)"
    );
}

/// Every care note is a complete sentence with a heading.
#[test]
fn care_notes_are_complete_sentences() {
    for (name, entry) in TABLE {
        for (topic, note) in entry.care {
            assert!(!topic.label().is_empty(), "{name}: empty topic heading");
            let Cited { value, .. } = note;
            assert!(
                value.ends_with('.') && value.len() > 8,
                "{name}: care note {value:?} is not a sentence"
            );
        }
    }
}
