//! Tests of the table rows, the sort, the filter, the copy text and the shared wording.

use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::material::{
    BuiltinMaterials, MaterialCatalogue, Toughness, built_in_gem_data,
};

use crate::{
    material_lookup::EditorMaterialLookup,
    material_reference::{
        Column, CustomGemData, DispersionScale, Filter, GemFacts, Origin, ReferenceRow,
        dispersion_chip_text, family_line, filter_counts, filter_rows, reference_rows, sort_rows,
        table_text,
        text::{DASH, sg_basis_note},
    },
};

const DASH_TEXT: &str = DASH;

const ALL_COLUMNS: [Column; 10] = [
    Column::Name,
    Column::CrystalSystem,
    Column::OpticCharacter,
    Column::Ri,
    Column::Birefringence,
    Column::Dispersion,
    Column::SpecificGravity,
    Column::Hardness,
    Column::Toughness,
    Column::Cleavage,
];

fn built_in_rows() -> Vec<ReferenceRow> {
    let catalogue = MaterialCatalogue::build(&[]);
    reference_rows(&catalogue, &BuiltinMaterials, &[])
}

fn custom_stone() -> GemMaterial {
    GemMaterial::new_custom("My Stone", 1.6, 0.012, 0.0, [0.9, 0.9, 0.9])
}

fn rows_with_custom(custom_gem: &[(String, CustomGemData)]) -> Vec<ReferenceRow> {
    let customs = [custom_stone()];
    let catalogue = MaterialCatalogue::build(&customs);
    let lookup = EditorMaterialLookup::new(&customs);
    reference_rows(&catalogue, &lookup, custom_gem)
}

fn row<'a>(rows: &'a [ReferenceRow], name: &str) -> &'a ReferenceRow {
    rows.iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("no row named {name}"))
}

/// T-E1: one row per catalogue material, with the origin and tags right.
#[test]
fn rows_cover_the_catalogue_with_origin_and_tags() {
    let rows = built_in_rows();
    let names: Vec<String> = GemMaterial::all_materials()
        .into_iter()
        .map(|gem| gem.name)
        .collect();
    assert_eq!(rows.len(), 39);
    assert_eq!(
        rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
        names
    );
    for row in &rows {
        let data = built_in_gem_data(&row.name).expect("every built-in has gem data");
        assert_eq!(row.origin, Origin::from(data.origin), "{}", row.name);
        assert_eq!(row.species, data.species, "{}", row.name);
    }
    assert_eq!(row(&rows, "Diamond").origin, Origin::Natural);
    assert_eq!(row(&rows, "Diamond").origin.tag(), "");
    assert_eq!(row(&rows, "Glass (N-BK7)").origin, Origin::Glass);
    assert_eq!(row(&rows, "Glass (N-BK7)").origin.tag(), "Glass");
    assert_eq!(row(&rows, "Synthetic Moissanite").origin, Origin::Synthetic);
    assert_eq!(row(&rows, "Synthetic Moissanite").origin.tag(), "Synthetic");
}

/// T-E1: the texts of one well-known row.
#[test]
fn diamond_row_texts() {
    let rows = built_in_rows();
    let diamond = row(&rows, "Diamond");
    assert_eq!(diamond.ri_text, "2.417");
    assert_eq!(diamond.birefringence_text, DASH_TEXT);
    assert_eq!(diamond.dispersion_bg_text, "0.044");
    assert_eq!(diamond.dispersion_fc_text, "0.026");
    assert_eq!(diamond.sg_text, "3.52");
    assert_eq!(diamond.crystal_system, "Cubic");
    assert_eq!(diamond.optic_character, "Isotropic");
    assert_eq!(diamond.gem.hardness.as_deref(), Some("10"));
    assert!(!diamond.gem.user_entered);
    let sapphire = row(&rows, "Sapphire");
    assert_eq!(sapphire.optic_character, "Uniaxial (\u{2212})");
    assert_eq!(sapphire.crystal_system, "Trigonal");
    assert_ne!(sapphire.birefringence_text, DASH_TEXT);
}

/// T-E1: a custom material gets a Custom row, "Not recorded" facts and its own gem data.
#[test]
fn custom_rows_read_not_recorded_or_their_own_data() {
    let plain = rows_with_custom(&[]);
    assert_eq!(plain.len(), 40);
    let mine = row(&plain, "My Stone");
    assert_eq!(mine.origin, Origin::Custom);
    assert_eq!(mine.origin.tag(), "Mine");
    assert!(mine.gem.user_entered);
    assert!(mine.gem.is_empty());
    assert_eq!(mine.gem.summary(), "Not recorded");
    assert_eq!(mine.sg_text, DASH_TEXT);
    assert_eq!(mine.birefringence_text, DASH_TEXT);
    assert!(mine.species.is_empty());

    let data = CustomGemData {
        mohs: Some((7.5, 7.0)),
        toughness: Some((Toughness::Good, Toughness::Fair)),
        cleavage: Some("Imperfect basal".to_owned()),
        fracture: Some("  ".to_owned()),
        care: Some("Avoid sudden temperature changes.".to_owned()),
        source: Some("My notes, 2026".to_owned()),
    };
    let filled = rows_with_custom(&[("my stone".to_owned(), data)]);
    let mine = row(&filled, "My Stone");
    assert_eq!(mine.gem.hardness.as_deref(), Some("7\u{2013}7.5"));
    assert_eq!(mine.gem.toughness.as_deref(), Some("Fair to good"));
    assert_eq!(mine.gem.cleavage.as_deref(), Some("Imperfect basal"));
    assert_eq!(mine.gem.fracture, None);
    assert_eq!(mine.gem.care, vec!["Avoid sudden temperature changes."]);
    assert_eq!(mine.gem.sources, vec!["Entered by you", "My notes, 2026"]);
    assert_eq!(
        mine.gem.summary(),
        "Mohs 7\u{2013}7.5 \u{b7} Fair to good \u{b7} Imperfect basal"
    );
    assert_eq!(mine.sort.hardness_high, Some(7.5));
    assert_eq!(mine.sort.hardness_low, Some(7.0));
}

/// T-E1: the card summaries for graded, ungraded and glass materials.
#[test]
fn facts_summaries_and_hardness_lines() {
    let topaz = GemFacts::for_material("Topaz", false, None);
    assert!(
        topaz.summary().starts_with("Mohs 8 \u{b7} "),
        "{}",
        topaz.summary()
    );
    assert_eq!(topaz.hardness_line().as_deref(), Some("8 (Mohs)"));
    assert!(
        topaz
            .cleavage
            .as_deref()
            .is_some_and(|c| c.starts_with("Perfect"))
    );
    assert_eq!(topaz.cleavage_rank, Some(7));
    assert!(!topaz.sources.is_empty());

    let glass = GemFacts::for_material("Glass (N-BK7)", false, None);
    assert_eq!(glass.toughness, None);
    assert!(
        glass.summary().contains("Not graded"),
        "{}",
        glass.summary()
    );
    let line = glass.hardness_line().expect("glass has a hardness");
    assert!(
        line.starts_with("about ") && line.ends_with("(Knoop)"),
        "{line}"
    );

    let unknown = GemFacts::for_material("Nothing", false, None);
    assert!(unknown.is_empty());
    assert_eq!(unknown.summary(), "Not recorded");

    let derived = GemFacts::for_material("Color-Change Garnet (Pyrope-Spessartine)", false, None);
    assert!(derived.notes.iter().any(|note| !note.is_empty()));
    assert!(sg_basis_note("Morganite").is_some_and(|note| note.starts_with("Derived")));
    assert_eq!(sg_basis_note("Diamond"), None);
}

/// U15: the cleavage warning covers Perfect, Good and Distinct on the strongest direction.
#[test]
fn cleavage_warns_for_distinct_or_better() {
    use crate::material_reference::facts::cleavage_warns;

    // Topaz: perfect basal cleavage.
    assert!(cleavage_warns(&GemFacts::for_material(
        "Topaz", false, None
    )));
    // Alexandrite: strongest direction distinct {110}.
    assert!(cleavage_warns(&GemFacts::for_material(
        "Alexandrite",
        false,
        None
    )));
    // No data, and a glass with no cleavage.
    assert!(!cleavage_warns(&GemFacts::default()));
    assert!(!cleavage_warns(&GemFacts::for_material(
        "Glass (N-BK7)",
        false,
        None
    )));
    // Every built-in: the warning agrees with the strongest rank (5 is Distinct).
    for row in built_in_rows() {
        assert_eq!(
            cleavage_warns(&row.gem),
            row.gem.cleavage_rank.is_some_and(|rank| rank >= 5),
            "{}",
            row.name
        );
    }

    let custom = |text: Option<&str>| {
        GemFacts::for_material(
            "Mine",
            true,
            Some(&CustomGemData {
                cleavage: text.map(str::to_owned),
                ..CustomGemData::default()
            }),
        )
    };
    assert!(cleavage_warns(&custom(Some("Perfect basal {001}"))));
    assert!(cleavage_warns(&custom(Some("  good in two directions"))));
    assert!(cleavage_warns(&custom(Some("DISTINCT, rhombohedral"))));
    assert!(!cleavage_warns(&custom(Some("Imperfect"))));
    assert!(!cleavage_warns(&custom(Some("None"))));
    assert!(!cleavage_warns(&custom(Some("not perfect"))));
    assert!(!cleavage_warns(&custom(None)));
}

/// The Stone tab chip's toughness text: a grade, a range, a directional range, and no grade.
#[test]
fn toughness_chip_text_for_grades_ranges_and_gaps() {
    use crate::material_reference::facts::toughness_chip_text;

    let chip = |name: &str| toughness_chip_text(&GemFacts::for_material(name, false, None));
    // The table grades Topaz Poor (GIA).
    assert_eq!(chip("Topaz").as_deref(), Some("Toughness Poor"));
    assert_eq!(chip("Sapphire").as_deref(), Some("Toughness Excellent"));
    // "Fair to good" and "Poor to fair" are ranges.
    assert_eq!(
        chip("Emerald").as_deref(),
        Some("Toughness Fair\u{2013}good")
    );
    assert_eq!(
        chip("Tanzanite").as_deref(),
        Some("Toughness Poor\u{2013}fair")
    );
    // Diamond is "Good to exceptional (by direction)"; the note stays off the chip.
    assert_eq!(
        chip("Diamond").as_deref(),
        Some("Toughness Good\u{2013}exceptional")
    );
    // No published grade.
    assert_eq!(chip("Glass (N-BK7)"), None);
    assert_eq!(toughness_chip_text(&GemFacts::default()), None);
    // A custom range.
    let custom = GemFacts::from_custom(&CustomGemData {
        toughness: Some((Toughness::Good, Toughness::VeryPoor)),
        ..CustomGemData::default()
    });
    assert_eq!(
        toughness_chip_text(&custom).as_deref(),
        Some("Toughness Very poor\u{2013}good")
    );
}

/// The sort key of `row` in `column` as the table prints it, `None` where the row has no value.
fn printed_key(row: &ReferenceRow, column: Column) -> Option<f64> {
    let parse = |text: &str| text.parse::<f64>().ok();
    match column {
        Column::Ri => parse(&row.ri_text),
        Column::Birefringence => parse(&row.birefringence_text),
        Column::Dispersion => parse(&row.dispersion_bg_text),
        Column::SpecificGravity => parse(&row.sg_text),
        Column::Hardness => row.gem.hardness_range.map(|(_, high)| f64::from(high)),
        Column::Toughness => row.gem.toughness_rank.map(f64::from),
        Column::Cleavage => row.gem.cleavage_rank.map(f64::from),
        Column::Name | Column::CrystalSystem | Column::OpticCharacter => None,
    }
}

fn text_key(row: &ReferenceRow, column: Column) -> String {
    match column {
        Column::CrystalSystem => row.crystal_system.to_lowercase(),
        Column::OpticCharacter => row.optic_character.to_lowercase(),
        _ => row.name.to_lowercase(),
    }
}

/// T-E2: every column, both directions: ordered, rows without a value last, ties by name.
#[test]
fn sort_orders_every_column_both_ways_with_missing_last() {
    let original = built_in_rows();
    for column in ALL_COLUMNS {
        for descending in [false, true] {
            let mut rows = original.clone();
            sort_rows(&mut rows, column, descending);
            assert_eq!(rows.len(), original.len());
            if matches!(
                column,
                Column::Name | Column::CrystalSystem | Column::OpticCharacter
            ) {
                for pair in rows.windows(2) {
                    let (a, b) = (text_key(&pair[0], column), text_key(&pair[1], column));
                    assert!(
                        if descending { a >= b } else { a <= b },
                        "{column:?} descending {descending}: {a} then {b}"
                    );
                }
                continue;
            }
            let keys: Vec<Option<f64>> = rows.iter().map(|r| printed_key(r, column)).collect();
            let present = keys.iter().take_while(|key| key.is_some()).count();
            assert!(
                keys[present..].iter().all(Option::is_none),
                "{column:?} descending {descending}: a missing value precedes a present one"
            );
            for pair in keys[..present].windows(2) {
                let (a, b) = (pair[0].unwrap_or(0.0), pair[1].unwrap_or(0.0));
                assert!(
                    if descending { a >= b } else { a <= b },
                    "{column:?} descending {descending}: {a} then {b}"
                );
            }
        }
    }
}

/// T-E2: the sort is deterministic whatever order the rows come in, and ties fall to the name.
#[test]
fn sort_is_deterministic_and_breaks_ties_by_name() {
    let mut forward = built_in_rows();
    let mut backward = built_in_rows();
    backward.reverse();
    for column in ALL_COLUMNS {
        for descending in [false, true] {
            sort_rows(&mut forward, column, descending);
            sort_rows(&mut backward, column, descending);
            assert_eq!(forward, backward, "{column:?} descending {descending}");
        }
    }
    let mut rows = built_in_rows();
    sort_rows(&mut rows, Column::CrystalSystem, false);
    for pair in rows.windows(2) {
        if pair[0].crystal_system == pair[1].crystal_system {
            assert!(pair[0].name.to_lowercase() <= pair[1].name.to_lowercase());
        }
    }
}

/// T-E2: pinned ends of the hardness and toughness sorts.
#[test]
fn hardness_and_toughness_sorts_start_where_expected() {
    let mut rows = built_in_rows();
    sort_rows(&mut rows, Column::Hardness, true);
    assert_eq!(rows[0].name, "Diamond");
    sort_rows(&mut rows, Column::Hardness, false);
    let softest = rows
        .iter()
        .filter_map(|r| r.gem.hardness_range.map(|(_, high)| high))
        .fold(f32::INFINITY, f32::min);
    assert!((rows[0].gem.hardness_range.expect("graded").1 - softest).abs() < f32::EPSILON);
    sort_rows(&mut rows, Column::Toughness, true);
    let last = rows.last().expect("rows");
    assert_eq!(last.gem.toughness_rank, None);
    // Benitoite and Titanite carry no toughness grade and stay behind every graded row.
    assert!(row(&rows, "Benitoite").gem.toughness.is_none());
}

/// T-E3: the filter chips, the search over name, species, system, optic character and cleavage,
/// and the counts.
#[test]
fn filter_and_search_cover_the_documented_fields() {
    let rows = rows_with_custom(&[]);
    let counts = filter_counts(&rows);
    assert_eq!(counts[0], 40);
    assert_eq!(counts[4], 1);
    assert_eq!(counts[1] + counts[2] + counts[3] + counts[4], counts[0]);
    assert_eq!(counts[3], 2);
    assert_eq!(filter_rows(&rows, Filter::All, "").len(), 40);
    assert_eq!(filter_rows(&rows, Filter::All, "   ").len(), 40);
    assert_eq!(filter_rows(&rows, Filter::Custom, "").len(), 1);
    assert_eq!(filter_rows(&rows, Filter::Glass, "").len(), counts[3]);
    assert_eq!(filter_rows(&rows, Filter::Natural, "").len(), counts[1]);
    assert_eq!(filter_rows(&rows, Filter::Synthetic, "").len(), counts[2]);

    let by_name = filter_rows(&rows, Filter::All, "TOPAZ");
    assert!(by_name.iter().any(|r| r.name == "Topaz"));
    assert_eq!(
        by_name.len(),
        filter_rows(&rows, Filter::All, "topaz").len()
    );

    let species = built_in_gem_data("Ruby")
        .expect("ruby")
        .species
        .to_lowercase();
    let by_species = filter_rows(&rows, Filter::All, &species);
    assert!(by_species.iter().any(|r| r.name == "Ruby"));

    let by_system = filter_rows(&rows, Filter::All, "trigonal");
    assert!(!by_system.is_empty());
    assert!(by_system.iter().all(|r| {
        r.crystal_system == "Trigonal"
            || r.name.to_lowercase().contains("trigonal")
            || r.species.to_lowercase().contains("trigonal")
            || r.optic_character.to_lowercase().contains("trigonal")
            || r.gem
                .cleavage
                .as_deref()
                .is_some_and(|c| c.to_lowercase().contains("trigonal"))
    }));

    let by_character = filter_rows(&rows, Filter::All, "biaxial");
    assert!(!by_character.is_empty());
    assert!(
        by_character
            .iter()
            .all(|r| r.optic_character.starts_with("Biaxial"))
    );

    let by_cleavage = filter_rows(&rows, Filter::All, "perfect");
    assert!(by_cleavage.iter().any(|r| r.name == "Topaz"));
    assert!(by_cleavage.iter().all(|r| {
        r.gem
            .cleavage
            .as_deref()
            .is_some_and(|c| c.to_lowercase().contains("perfect"))
            || r.name.to_lowercase().contains("perfect")
    }));

    assert_eq!(filter_rows(&rows, Filter::Glass, "n-bk7").len(), 1);
    assert!(filter_rows(&rows, Filter::Natural, "n-bk7").is_empty());
    assert!(filter_rows(&rows, Filter::All, "no such material").is_empty());
}

/// T-E4: the header, one line per row, ten fields per line, and the B-G / F-C switch.
#[test]
fn table_text_has_a_header_and_switches_scale() {
    let rows = built_in_rows();
    let visible = filter_rows(&rows, Filter::All, "diamond");
    let bg = table_text(&visible, DispersionScale::BG);
    let lines: Vec<&str> = bg.lines().collect();
    assert_eq!(
        lines[0],
        "Material\tCrystal System\tOptic Character\tRI\tBiref.\tDisp. (B\u{2013}G)\tSG\tHardness\tToughness\tCleavage"
    );
    assert_eq!(lines.len(), visible.len() + 1);
    assert!(bg.ends_with('\n'));
    for line in &lines {
        assert_eq!(line.split('\t').count(), 10, "{line}");
    }
    let diamond: Vec<&str> = lines
        .iter()
        .find(|line| line.starts_with("Diamond\t"))
        .expect("a Diamond line")
        .split('\t')
        .collect();
    assert_eq!(diamond[3], "2.417");
    assert_eq!(diamond[4], DASH_TEXT);
    assert_eq!(diamond[5], "0.044");
    assert_eq!(diamond[6], "3.52");
    assert_eq!(diamond[7], "10");

    let fc = table_text(&visible, DispersionScale::FC);
    assert!(
        fc.lines()
            .next()
            .is_some_and(|l| l.contains("Disp. (F\u{2013}C)"))
    );
    let fc_diamond = fc
        .lines()
        .find(|line| line.starts_with("Diamond\t"))
        .expect("a Diamond line");
    assert_eq!(fc_diamond.split('\t').nth(5), Some("0.026"));

    let none: Vec<&ReferenceRow> = Vec::new();
    assert_eq!(table_text(&none, DispersionScale::BG).lines().count(), 1);

    let glass = filter_rows(&rows, Filter::Glass, "");
    let text = table_text(&glass, DispersionScale::BG);
    assert!(
        text.contains(DASH_TEXT),
        "a glass has no toughness grade: {text}"
    );
}

/// T-E10: the chip text is the B-G figure to three decimals.
#[test]
fn dispersion_chip_text_is_the_bg_figure() {
    let diamond = GemMaterial::by_name("Diamond").expect("diamond");
    assert_eq!(dispersion_chip_text(&diamond), "0.044");
    let sapphire = GemMaterial::by_name("Sapphire").expect("sapphire");
    assert_eq!(dispersion_chip_text(&sapphire), "0.018");
    assert_ne!(
        dispersion_chip_text(&diamond),
        format!("{:.3}", diamond.dispersion.delta_f_c())
    );
    let tooltip = crate::material_reference::text::dispersion_chip_tooltip(&diamond);
    assert!(tooltip.contains("F\u{2013}C, is 0.026"), "{tooltip}");
}

/// T-E10: the family line is the one text the desktop and the mobile Stone tab show.
#[test]
fn family_line_keeps_the_stone_tab_format() {
    let diamond = GemMaterial::by_name("Diamond").expect("diamond");
    assert_eq!(
        family_line(&diamond, Origin::Natural),
        "Cubic \u{b7} isotropic \u{b7} built-in"
    );
    let sapphire = GemMaterial::by_name("Sapphire").expect("sapphire");
    assert_eq!(
        family_line(&sapphire, Origin::Natural),
        "Trigonal \u{b7} uniaxial (-) \u{b7} built-in"
    );
    assert!(family_line(&sapphire, Origin::Synthetic).ends_with("built-in"));
    assert!(family_line(&custom_stone(), Origin::Custom).ends_with("custom"));
}

/// The window's phrases from the string table.
#[test]
fn window_phrases_match_the_string_table() {
    use crate::material_reference::text::{copied_text, footer_text, subtitle};
    assert_eq!(
        subtitle(39, 0),
        "39 built-in materials. Click a column heading to sort; open a row for its curves, spectra and colors."
    );
    assert!(subtitle(39, 2).starts_with("39 built-in materials and 2 of your own. Click"));
    assert_eq!(
        footer_text(41, Column::Hardness, true),
        "41 materials, hardest first. Values are typical for each species; a dash means not recorded."
    );
    assert_eq!(copied_text(3), "Copied 3 materials as a table.");
}
