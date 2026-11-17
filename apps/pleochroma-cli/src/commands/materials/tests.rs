use crate::{outcome::EXIT_USAGE, testing};
use serde_json::Value;

/// The data lines of the text table: header and rule skipped, the footer dropped.
fn table_rows(text: &str) -> Vec<&str> {
    text.lines()
        .skip(2)
        .take_while(|line| !line.is_empty())
        .collect()
}

#[test]
fn the_table_lists_the_thirty_nine_built_ins_with_a_header_and_a_footer() {
    let outcome = testing::run(&["materials"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let rows = table_rows(&outcome.stdout);
    assert_eq!(rows.len(), 39, "{}", outcome.stdout);
    let header = outcome.stdout.lines().next().expect("a header");
    for word in [
        "Material",
        "RI",
        "Biref.",
        "Disp. (B\u{2013}G)",
        "SG",
        "Hardness",
    ] {
        assert!(header.contains(word), "{header}");
    }
    assert!(
        outcome.stdout.contains("39 materials."),
        "{}",
        outcome.stdout
    );
    assert!(outcome.stdout.lines().all(|line| line == line.trim_end()));
}

/// T-L3: sorting by hardness, hardest first, starts with Diamond.
#[test]
fn sorting_by_hardness_descending_starts_with_diamond() {
    let outcome = testing::run(&["materials", "--sort", "hardness", "--desc"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let rows = table_rows(&outcome.stdout);
    assert!(rows[0].starts_with("Diamond"), "{}", outcome.stdout);
    assert!(!rows[rows.len() - 1].starts_with("Diamond"));
}

#[test]
fn the_dispersion_option_switches_the_column_heading() {
    let bg = testing::run(&["materials", "--dispersion", "bg"]);
    let fc = testing::run(&["materials", "--dispersion", "fc"]);
    let heading = |text: &str| text.lines().next().expect("a header").to_string();
    assert!(heading(&bg.stdout).contains("B\u{2013}G"));
    assert!(heading(&fc.stdout).contains("F\u{2013}C"));
    assert_ne!(bg.stdout, fc.stdout);
}

#[test]
fn the_filter_keeps_only_the_glasses() {
    let outcome = testing::run(&["materials", "--filter", "glass", "--csv"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let lines: Vec<&str> = outcome.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", outcome.stdout);
    assert!(lines[1].starts_with("Glass ("), "{}", lines[1]);
    let custom = testing::run(&["materials", "--filter", "custom"]);
    assert!(custom.stdout.contains("0 materials."), "{}", custom.stdout);
}

#[test]
fn the_csv_has_a_header_and_a_line_for_every_material() {
    let outcome = testing::run(&["materials", "--csv"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let lines: Vec<&str> = outcome.stdout.lines().collect();
    assert_eq!(lines.len(), 40, "{}", outcome.stdout);
    assert!(lines[0].starts_with("name,origin,species,"), "{}", lines[0]);
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Topaz,natural,Topaz,")),
        "{}",
        outcome.stdout
    );
}

#[test]
fn the_json_table_carries_full_citations_and_addresses() {
    let outcome = testing::run(&["materials", "--json"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
    assert_eq!(value["count"], 39);
    let materials = value["materials"].as_array().expect("a list");
    assert_eq!(materials.len(), 39);
    for material in materials {
        let links = material["gem_data"]["source_links"]
            .as_array()
            .expect("source links");
        assert!(!links.is_empty(), "{}", material["name"]);
        for link in links {
            assert!(
                link["citation"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{link}"
            );
            if let Some(url) = link["url"].as_str() {
                assert!(url.starts_with("http"), "{url}");
            }
        }
    }
    let topaz = materials
        .iter()
        .find(|material| material["name"] == "Topaz")
        .expect("Topaz");
    let urls: Vec<&str> = topaz["gem_data"]["source_links"]
        .as_array()
        .expect("links")
        .iter()
        .filter_map(|link| link["url"].as_str())
        .collect();
    assert!(
        urls.contains(&"https://www.gia.edu/topaz-care-cleaning"),
        "{urls:?}"
    );
    assert_eq!(topaz["origin"], "natural");
    assert!(topaz["optics"]["dispersion_bg"].as_f64().is_some());
}

#[test]
fn the_markdown_tables_are_tables() {
    for (table, header) in [
        ("optics", "| Material | Optical character | n_D |"),
        ("gem-data", "| Material | Mohs | Toughness | Cleavage |"),
        ("built-in", "| Material | n_D | Birefringence"),
    ] {
        let outcome = testing::run(&["materials", "--markdown", table]);
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        let lines: Vec<&str> = outcome.stdout.lines().collect();
        assert_eq!(lines.len(), 41, "{table}: {}", outcome.stdout);
        assert!(lines[0].starts_with(header), "{}", lines[0]);
        assert!(lines[1].starts_with("|---|"), "{}", lines[1]);
        let columns = lines[0].matches(" | ").count();
        assert!(
            lines[2..]
                .iter()
                .all(|line| line.matches(" | ").count() == columns),
            "{table}"
        );
    }
    let gem = testing::run(&["materials", "--markdown", "gem-data"]);
    let topaz = gem
        .stdout
        .lines()
        .find(|line| line.starts_with("| Topaz |"))
        .expect("a Topaz row");
    assert!(topaz.contains("| 8 |"), "{topaz}");
    assert!(topaz.contains("Perfect basal {001}"), "{topaz}");
}

#[test]
fn the_sources_table_gives_every_address_and_the_notes() {
    let outcome = testing::run(&["materials", "--markdown", "sources"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    assert!(outcome.stdout.starts_with("| Source | Address |"));
    assert!(
        outcome
            .stdout
            .contains("https://www.gia.edu/topaz-care-cleaning"),
        "{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("Notes where sources differ"),
        "{}",
        outcome.stdout
    );
}

/// T-L3: the details of Topaz with its bands.
#[test]
fn one_material_with_its_bands_lists_them() {
    let outcome = testing::run(&["materials", "Topaz", "--bands"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let text = &outcome.stdout;
    assert!(text.starts_with("Topaz\n"), "{text}");
    for heading in [
        "Optics",
        "Colors (7 mm stone, face up)",
        "Color variants",
        "Absorption bands",
        "Durability and weight",
    ] {
        assert!(text.contains(heading), "{heading} in {text}");
    }
    let bands = text
        .split("Absorption bands")
        .nth(1)
        .and_then(|rest| rest.split("Durability").next())
        .expect("the band section");
    assert!(bands.contains(" nm  "), "{bands}");
    assert!(bands.contains("peak"), "{bands}");
    assert!(text.contains("Dispersion (B\u{2013}G)"), "{text}");
    assert!(text.contains("Gem data:"), "{text}");
    // Without --bands the section is not printed.
    let plain = testing::run(&["materials", "Topaz"]);
    assert!(!plain.stdout.contains("Absorption bands"));
}

#[test]
fn every_built_in_describes_itself_in_text_and_json() {
    let names = testing::run(&["materials", "--csv"]);
    for line in names.stdout.lines().skip(1) {
        let name = line.split(',').next().expect("a name");
        let text = testing::run(&["materials", name, "--bands"]);
        assert_eq!(text.exit, 0, "{name}: {}", text.stderr);
        let json = testing::run(&["materials", name, "--json"]);
        assert_eq!(json.exit, 0, "{name}: {}", json.stderr);
        let value: Value = serde_json::from_str(&json.stdout).expect("valid JSON");
        assert_eq!(value["name"], name);
        let lower = text.stdout.to_ascii_lowercase();
        for word in ["fluoresc", "ultraviolet", "phosphoresc"] {
            assert!(!lower.contains(word), "{name}: {word}");
        }
    }
}

#[test]
fn a_name_is_matched_in_any_letter_case_and_an_unknown_one_lists_the_choices() {
    let outcome = testing::run(&["materials", "cubic", "zirconia"]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    assert!(outcome.stdout.starts_with("Cubic Zirconia\n"));
    let unknown = testing::run(&["materials", "Unobtainium"]);
    assert_eq!(unknown.exit, EXIT_USAGE);
    assert!(unknown.stderr.contains("Diamond"), "{}", unknown.stderr);
}

#[test]
fn the_curves_file_has_a_column_per_direction() {
    let path = testing::temp_path("sapphire-curves.csv");
    let path = path.to_string_lossy().into_owned();
    let outcome = testing::run(&["materials", "Sapphire", "--curves", &path]);
    assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
    let curves = testing::file_text(&outcome, &path).expect("the curves file");
    let lines: Vec<&str> = curves.lines().collect();
    assert_eq!(lines[0], "wavelength_nm,n_omega,n_epsilon");
    assert_eq!(lines.len(), 1 + 75);
    assert!(lines[1].starts_with("380,"), "{}", lines[1]);
    assert!(lines[75].starts_with("750,"), "{}", lines[75]);
    let index = |line: &str| -> f64 {
        line.split(',')
            .nth(1)
            .expect("an index")
            .parse()
            .expect("a number")
    };
    // The index falls with wavelength.
    assert!(index(lines[1]) > index(lines[75]));

    let diamond = testing::temp_path("diamond-curves.csv");
    let diamond = diamond.to_string_lossy().into_owned();
    let outcome = testing::run(&["materials", "Diamond", "--curves", &diamond]);
    let curves = testing::file_text(&outcome, &diamond).expect("the curves file");
    assert_eq!(curves.lines().next(), Some("wavelength_nm,n"));
}

#[test]
fn the_report_can_be_written_to_a_file() {
    let target = testing::temp_path("materials-table.txt");
    let target = target.to_string_lossy().into_owned();
    let written = testing::run(&["materials", "--out", &target]);
    assert_eq!(written.exit, 0, "{}", written.stderr);
    assert_eq!(written.stdout, "");
    let printed = testing::run(&["materials"]);
    assert_eq!(
        testing::file_text(&written, &target),
        Some(printed.stdout.as_str())
    );
}

#[test]
fn the_output_is_the_same_every_time() {
    for line in [
        &["materials"][..],
        &["materials", "--json"],
        &["materials", "Peridot", "--bands"],
    ] {
        assert_eq!(testing::run(line).stdout, testing::run(line).stdout);
    }
}
