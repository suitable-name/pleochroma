//! Tests for the printed layout: the header block, Facet Data, Size Data, Design Data, the four
//! views, comments, the Pavilion/Crown split, [`SheetDetails`] and the text sheet.
//!
//! Every expected number is worked out by hand from the fixtures' own tier tables.

use super::{
    CutViews, SheetDetails, SheetLayout, VECTOR_VIEW_HEIGHT, VECTOR_VIEW_WIDTH,
    cutting_sheet_document, cutting_sheet_document_with, cutting_sheet_html,
    cutting_sheet_html_with, cutting_sheet_text_with, month_year_text, render_cut_views,
    render_vector_cut_views,
    summary::smaller_round_sizes,
    tests::{label_cells, round_brilliant_design, section_html},
};
use pleochroma_cut_core::{
    Design, MaterialSelection, PreformSpec, ScheduleMeta, native::DesignMetadata,
};
use pleochroma_solid::diagram2d::{LineWeight, view_to_svg};

/// Byte positions of `needles` in `haystack`, each searched after the previous one, so the
/// result is ascending exactly when the needles appear in that order.
fn positions_in_order(haystack: &str, needles: &[&str]) -> Vec<usize> {
    let mut from = 0;
    needles
        .iter()
        .map(|needle| {
            let at = from
                + haystack[from..]
                    .find(needle)
                    .unwrap_or_else(|| panic!("{needle:?} missing or out of order"));
            from = at + needle.len();
            at
        })
        .collect()
}

// ---- Facet Data ----------------------------------------------------------------------

/// The Standard Round Brilliant of the fixture, counted by hand. Pavilion: Pavilion Main 8 +
/// Lower Girdle 16 + Culet 1 (an empty index list is one facet) = 25 facets in 3 tiers.
/// Girdle: 16 facets in 1 tier. Crown: Star 8 + Crown Main 8 + Upper Girdle 16 = 32 facets in
/// 3 tiers, plus the table (1 facet, 1 tier). Totals: 3 + 1 + 3 + 1 = 8 tiers and
/// 25 + 16 + 32 + 1 = 74 facets.
#[test]
fn facet_data_of_the_standard_round_brilliant_is_counted_by_hand() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let facets = layout.facets;
    assert_eq!((facets.pavilion.tiers, facets.pavilion.facets), (3, 25));
    assert_eq!((facets.girdle.tiers, facets.girdle.facets), (1, 16));
    assert_eq!((facets.crown.tiers, facets.crown.facets), (3, 32));
    assert_eq!((facets.table.tiers, facets.table.facets), (1, 1));
    assert_eq!(facets.total_tiers(), 8);
    assert_eq!(facets.total_facets(), 74);
}

/// The crown reads `N+1` when the design has a table: its facets plus the table. The template
/// prints facets on the left half and tiers on the right.
#[test]
fn the_crown_reads_n_plus_one_when_there_is_a_table() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    assert_eq!(layout.facets.crown_facets_text(), "32+1");
    assert_eq!(layout.facets.crown_tiers_text(), "3+1");

    let html = cutting_sheet_html(&design, &solved, None, &[]);
    for row in [
        "<tr><td>Pavilion facets</td><td class=\"split\">25</td><td>Pavilion tiers</td><td>3</td></tr>",
        "<tr><td>Girdle facets</td><td class=\"split\">16</td><td>Girdle tiers</td><td>1</td></tr>",
        "<tr><td>Crown facets</td><td class=\"split\">32+1</td><td>Crown tiers</td><td>3+1</td></tr>",
        "<tr><td>Total facets</td><td class=\"split\">74</td><td>Total tiers</td><td>8</td></tr>",
    ] {
        assert!(html.contains(row), "{row}\n{html}");
    }
}

/// The concave fixture has no table, so the crown prints a plain count. Pavilion: flat -42
/// (4) + flat -38 (4) + the groove's 8 placements = 16 facets in 3 tiers. Girdle: 4 facets in
/// 1 tier. Crown: flat 32 (4) + flat 40 (4) + the dimple's 4 placements = 12 facets in 3
/// tiers. Totals: 7 tiers and 16 + 4 + 12 = 32 facets.
#[test]
fn facet_data_of_the_concave_fixture_counts_each_tool_by_its_placements() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let facets = layout.facets;
    assert_eq!((facets.pavilion.tiers, facets.pavilion.facets), (3, 16));
    assert_eq!((facets.girdle.tiers, facets.girdle.facets), (1, 4));
    assert_eq!((facets.crown.tiers, facets.crown.facets), (3, 12));
    assert_eq!((facets.table.tiers, facets.table.facets), (0, 0));
    assert_eq!(facets.total_tiers(), 7);
    assert_eq!(facets.total_facets(), 32);
    assert_eq!(facets.crown_facets_text(), "12", "no table, no +1");
    assert_eq!(facets.crown_tiers_text(), "3");
}

// ---- Size Data -----------------------------------------------------------------------

/// The six ratios are the editor's own: length, crown and pavilion from
/// `Design::stone_proportions`, volume and height from `Design::measure`, and P/C is
/// `(P/W) / (C/W)`.
#[test]
fn size_data_matches_the_existing_proportion_functions() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let sizes = SheetLayout::build(&design, &solved, &[], &SheetDetails::default()).sizes;
    let proportions = design
        .stone_proportions(&solved)
        .expect("the round brilliant closes");
    let metrics = design
        .measure()
        .expect("solves")
        .expect("the round brilliant measures");
    let width = metrics.width_axis;
    let close = |a: Option<f64>, b: f64| {
        let a = a.expect("measured");
        assert!((a - b).abs() < 1e-12, "{a} vs {b}");
    };
    close(sizes.length_to_width, proportions.length_to_width.unwrap());
    close(sizes.height_to_width, metrics.total_height / width);
    close(
        sizes.volume_to_width_cubed,
        metrics.volume / (width * width * width),
    );
    close(
        sizes.pavilion_to_width,
        proportions.pavilion_to_width_percent.unwrap() / 100.0,
    );
    close(
        sizes.crown_to_width,
        proportions.crown_to_width_percent.unwrap() / 100.0,
    );
    close(
        sizes.pavilion_to_crown,
        proportions.pavilion_depth.unwrap() / proportions.crown_height.unwrap(),
    );
    // A round stone: length over width is 1.000 (the cut-core proportions test pins 1e-6).
    assert_eq!(sizes.cells()[0], ("L/W", "1.000".to_string()));
}

/// The cells are in print order, three decimals, and the HTML prints L/W, H/W, V/W³ on the left
/// half and P/W, C/W, P/C on the right.
#[test]
fn size_data_prints_six_cells_with_three_decimals() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let cells = layout.sizes.cells();
    let labels: Vec<&str> = cells.iter().map(|(label, _)| *label).collect();
    assert_eq!(labels, ["L/W", "H/W", "V/W\u{b3}", "P/W", "C/W", "P/C"]);
    for (label, value) in &cells {
        let decimals = value
            .split('.')
            .nth(1)
            .unwrap_or_else(|| panic!("{label}: {value}"));
        assert_eq!(decimals.len(), 3, "{label} {value}");
    }
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let row = format!(
        "<tr><td>V/W\u{b3}</td><td class=\"split\">{}</td><td>P/C</td><td>{}</td></tr>",
        cells[2].1, cells[5].1
    );
    assert!(html.contains(&row), "{row}\n{html}");
}

/// A design that does not measure shows a dash in every cell.
#[test]
fn size_data_is_dashes_when_the_design_does_not_measure() {
    let design = Design::new(
        PreformSpec::block(1.0, 1.0, 1.0),
        ScheduleMeta::default(),
        Vec::new(),
    );
    let solved = design.solve().expect("no tiers to solve");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    assert!(layout.sizes.cells().iter().all(|(_, value)| value == "-"));
}

// ---- Design Data ---------------------------------------------------------------------

/// Without the optional fields: the template's five rows, the RI range row giving the design's
/// own index with its material, a dash for the size range and the shape (nothing invented),
/// the symmetry and the index gear.
#[test]
fn design_data_without_the_optional_fields() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Sapphire".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.76),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let rows: Vec<(&str, &str)> = layout
        .design_data
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("RI range", "1.760 (Sapphire)"),
            ("Size range", "-"),
            ("Symmetry", "8-fold, mirror"),
            ("Index gear", "96 Index"),
            ("Shape", "-"),
        ]
    );
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(
        html.contains("<tr><td class=\"split\">RI range</td><td>1.760 (Sapphire)</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<tr><td class=\"split\">Index gear</td><td>96 Index</td></tr>"),
        "{html}"
    );
}

/// With every optional field: the RI range reads "low to high", the size range follows, the
/// shape comes last, and a design without mirror symmetry reads "4-fold".
#[test]
fn design_data_with_the_optional_fields() {
    let mut design = round_brilliant_design();
    design.meta.symmetry_order = 4;
    design.meta.mirror = false;
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        shape: "Round".to_string(),
        ri_range: Some((1.54, 1.76)),
        size_range_mm: Some((4.0, 12.5)),
        ..SheetDetails::default()
    };
    let layout = SheetLayout::build(&design, &solved, &[], &details);
    let rows: Vec<(&str, &str)> = layout
        .design_data
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("RI range", "1.54 to 1.76"),
            ("Size range", "4 - 12.5 mm"),
            ("Symmetry", "4-fold"),
            ("Index gear", "96 Index"),
            ("Shape", "Round"),
        ]
    );
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    for row in [
        "<tr><td class=\"split\">RI range</td><td>1.54 to 1.76</td></tr>",
        "<tr><td class=\"split\">Size range</td><td>4 - 12.5 mm</td></tr>",
        "<tr><td class=\"split\">Shape</td><td>Round</td></tr>",
    ] {
        assert!(html.contains(row), "{row}\n{html}");
    }
}

/// Without a size range the Size range row gives the design's own size, its girdle diameter,
/// and never a range made up from it.
#[test]
fn the_size_range_falls_back_to_the_girdle_diameter() {
    let mut design = round_brilliant_design();
    design.girdle_diameter_mm = Some(6.5);
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    assert_eq!(
        layout.design_data[1],
        ("Size range".to_string(), "6.5 mm".to_string())
    );
}

/// The size in millimetres and the carat estimate are in the Stone summary of the additional
/// information, after the template's Design Data; the text sheet keeps its girdle diameter and
/// carat lines after Design Data.
#[test]
fn the_size_and_carat_lines_follow_design_data() {
    let mut design = round_brilliant_design();
    design.girdle_diameter_mm = Some(6.5);
    design.material.name = Some("Diamond".to_string());
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    positions_in_order(
        &html,
        &[
            "Design Data",
            "<h2 class=\"extra\">Additional information</h2>",
            "<h3>Stone summary</h3>",
            "<tr><th>Size (W \u{d7} L \u{d7} D)</th>",
            "<tr><th>Carat weight (estimate)</th>",
        ],
    );
    let text = cutting_sheet_text_with(&design, &solved, &[], &SheetDetails::default());
    positions_in_order(
        &text,
        &[
            "\nDesign Data\n",
            "  Girdle diameter: 6.500 mm\n",
            "  Carat weight (estimate): ",
        ],
    );
}

// ---- Header, comments, order ---------------------------------------------------------

/// Title as the heading, then the subtitle, then Author | Month and year (no "by": the template
/// labels the cell by its place), each in its own line of the template.
#[test]
fn the_header_prints_title_subtitle_author_and_date_in_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Starfire & Co".to_string(),
        subtitle: "Suite No. 3".to_string(),
        author: "A. Cutter".to_string(),
        date_text: "October 2026".to_string(),
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    positions_in_order(
        &html,
        &[
            "<h1 class=\"title\">Starfire &amp; Co</h1>",
            "<div class=\"rule-thick\"></div>",
            "<p class=\"subtitle\">Suite No. 3</p>",
            "<div class=\"rule-thin\"></div>",
            "<p class=\"author\">A. Cutter</p><p class=\"date\">October 2026</p>",
            "<h2 class=\"block\">Facet Data</h2>",
        ],
    );
    assert!(html.contains("<title>Starfire &amp; Co</title>"));
}

/// No title: the heading is "Cutting instructions"; empty subtitle, author and date keep their
/// lines of the template, blank.
#[test]
fn an_empty_header_keeps_the_default_heading_and_blank_lines() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &SheetDetails::default());
    assert!(html.contains("<h1 class=\"title\">Cutting instructions</h1>"));
    assert!(html.contains("<title>Cutting instructions</title>"));
    for class in ["subtitle", "author", "date"] {
        assert!(
            html.contains(&format!("<p class=\"{class}\">&nbsp;</p>")),
            "{class}"
        );
    }
    assert!(
        !html.contains("<h2 class=\"block\">Comments</h2>"),
        "no comments, no block"
    );
}

/// The blocks follow the template: the left column (title, Facet Data, Size Data, Design Data),
/// the views, Comments, Pavilion, Crown, then the additional information -- in the HTML and,
/// without the views, in the text sheet.
#[test]
fn the_blocks_follow_the_template_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Order".to_string(),
        comments: vec!["Polish on a tin lap.".to_string()],
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    positions_in_order(
        &html,
        &[
            "<header class=\"screen-bar\">",
            "<main class=\"template\">",
            "<h1 class=\"title\">Order</h1>",
            "<h2 class=\"block\">Facet Data</h2>",
            "<h2 class=\"block\">Size Data</h2>",
            "<h2 class=\"block\">Design Data</h2>",
            "<div class=\"views\">",
            "<h2 class=\"block\">Comments</h2>",
            "<h2 class=\"section\">Pavilion</h2>",
            "<h2 class=\"section\">Crown</h2>",
            "</main>",
            "<h2 class=\"extra\">Additional information</h2>",
            "<h3>Stone summary</h3>",
            "<h3>Masts</h3>",
        ],
    );
    // The comment is in the template's Comments only: the additional information repeats no note.
    assert_eq!(html.matches("Polish on a tin lap.").count(), 1);

    let text = cutting_sheet_text_with(&design, &solved, &[], &details);
    positions_in_order(
        &text,
        &[
            "Order\n",
            "\nFacet Data\n",
            "\nSize Data\n",
            "\nDesign Data\n",
            "\nComments\n",
            "\nPavilion\n",
            "\nCrown\n",
        ],
    );
}

/// Every comment line is printed, escaped, centred under the Comments heading.
#[test]
fn every_comment_line_is_printed() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        comments: vec!["First <b>".to_string(), "Second".to_string()],
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    assert!(html.contains("<p>First &lt;b&gt;</p>"), "{html}");
    assert!(html.contains("<p>Second</p>"), "{html}");
    let text = cutting_sheet_text_with(&design, &solved, &[], &details);
    assert!(text.contains("  First <b>\n  Second\n"), "{text}");
}

// ---- Sections ------------------------------------------------------------------------

/// The girdle rows are in the Pavilion section, the table is the last row of the Crown
/// section, and the step numbers run on across both.
#[test]
fn the_sections_split_by_block_with_the_girdle_in_the_pavilion_and_the_table_last() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let pavilion: Vec<&str> = layout
        .pavilion_rows()
        .iter()
        .map(|(row, _)| row.label())
        .collect();
    let crown: Vec<&str> = layout
        .crown_rows()
        .iter()
        .map(|(row, _)| row.label())
        .collect();
    assert_eq!(pavilion, ["G1", "P1", "P2", "Culet"]);
    assert_eq!(crown, ["C1", "C2", "C3", "T"]);
    let steps: Vec<usize> = layout
        .pavilion_rows()
        .iter()
        .chain(&layout.crown_rows())
        .map(|(row, _)| row.sequence)
        .collect();
    assert_eq!(steps, [1, 2, 3, 4, 5, 6, 7, 8]);

    let text = layout.to_text();
    let girdle_at = text.find(" G1 ").expect("G1 row in the text sheet");
    let crown_at = text.find("\nCrown\n").expect("Crown heading");
    assert!(
        girdle_at < crown_at,
        "G1 is printed before the Crown heading"
    );
}

/// The section tables have the template's four columns -- code, angle, index list,
/// instruction -- and nothing else: no step number, no block, no mast.
#[test]
fn the_section_tables_have_the_template_columns() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert_eq!(
        html.matches(
            "<colgroup><col class=\"code\"><col class=\"angle\"><col class=\"indices\">\
             <col class=\"instruction\"></colgroup>"
        )
        .count(),
        2,
        "one per section"
    );
    for section in [
        section_html(&html, "Pavilion"),
        section_html(&html, "Crown"),
    ] {
        assert!(!section.contains("<th"), "no header row: {section}");
        for row in section.split("<tr").skip(1) {
            assert_eq!(row.matches("<td").count(), 4, "{row}");
        }
    }
}

/// The section lines carry the template's columns: code, `40.00°`, the index list and the
/// instruction, banded from the first tier, the table reading `Table`.
#[test]
fn the_section_lines_are_the_template_columns() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let crown = layout.crown_lines();
    let table = crown.last().expect("the table is the last crown line");
    assert_eq!(table.code, "T");
    assert_eq!(table.angle, "0.00\u{b0}");
    assert_eq!(table.indices, "Table");
    assert!(!table.tool);
    let shading: Vec<bool> = layout
        .pavilion_lines()
        .iter()
        .map(|line| line.shaded)
        .collect();
    assert_eq!(shading, [true, false, true, false]);
}

/// The print rules set US Letter with the template's margins, print the grey bands, and keep a
/// tier's lines together.
#[test]
fn the_print_rules_set_letter_and_keep_tiers_together() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains("@page { size: letter; margin: 0.4in; }"));
    assert!(html.contains("print-color-adjust: exact;"));
    assert!(html.contains("table.rows tr.shaded td { background: #d9d9d9; }"));
    assert!(html.contains("table.rows tbody { break-inside: avoid; page-break-inside: avoid; }"));
}

// ---- Screen view and additional information ----------------------------------------------

/// The screen-only bar (the heading, then "Cutting instructions · Pleochroma") comes right after
/// `<body>`, before the template's `<main>` and outside it, so the printed template is unchanged.
#[test]
fn the_screen_bar_is_outside_the_template() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Bar & Co".to_string(),
        ..SheetDetails::default()
    };
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    let bar = "<header class=\"screen-bar\"><span class=\"bar-title\">Bar &amp; Co</span>\
               <span class=\"bar-what\">Cutting instructions \u{b7} Pleochroma</span></header>";
    positions_in_order(
        &html,
        &[
            "<body>\n",
            bar,
            "<main class=\"template\">",
            "</main>",
            "<section class=\"extra\">",
        ],
    );
    assert_eq!(html.matches("<header").count(), 1);
    let template = html
        .split("<main class=\"template\">")
        .nth(1)
        .and_then(|rest| rest.split("</main>").next())
        .expect("the template");
    assert!(!template.contains("screen-bar"), "{template}");
    assert!(!template.contains("Pleochroma"), "{template}");
}

/// On screen every printed page is a white Letter sheet with a shadow on a grey ground; the print
/// rules hide the screen bar and take the sheet look away again (white, no shadow, no padding, no
/// minimum height), so the printout is the plain Letter pages with the `@page` margins.
#[test]
fn print_hides_the_screen_only_parts() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let (screen, print) = html
        .split_once("@media print {")
        .expect("the sheet has print rules");
    let print = print
        .split("</style>")
        .next()
        .expect("the stylesheet closes");
    for rule in [
        "header.screen-bar {",
        "main.template, section.extra {",
        "width: 8.5in; min-height: 11in;",
        "box-shadow: 0 1px 3px",
        "background: #e5e7eb;",
    ] {
        assert!(screen.contains(rule), "screen rule {rule}");
    }
    for rule in [
        "header.screen-bar { display: none; }",
        "body { margin: 0; padding: 0; background: #ffffff; }",
        "width: auto; min-height: 0; margin: 0; padding: 0; background: #ffffff; box-shadow: none;",
    ] {
        assert!(print.contains(rule), "print rule {rule}\n{print}");
    }
    // The additional information still starts a printed page of its own.
    assert!(html.contains("section.extra { break-before: page; page-break-before: always; }"));
}

/// The mast table has the template's look: a rule under the header, the grey bands of the
/// Pavilion and Crown lines from the first row, the figures right-aligned. Its rows keep their
/// plain cells.
#[test]
fn the_mast_table_has_the_template_bands() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    for rule in [
        "table.masts tbody tr:nth-child(odd) td { background: #d9d9d9; }",
        "padding: 0 6pt 2pt; border-bottom: 1pt solid #000000;",
        "table.masts th:nth-child(n+3), table.masts td:nth-child(n+3) {\n    text-align: right;",
    ] {
        assert!(html.contains(rule), "{rule}");
    }
    assert!(html.contains("<table class=\"masts\">\n<thead><tr><th>Code</th><th>Tier</th>"));
}

/// The round brilliant fixture in diamond at a 6.5 mm girdle: the Stone summary gives the size
/// in millimetres (the girdle diameter times 1, L/W and H/W), the proportions, the material and
/// the carat estimate at 6.5 mm with its cube-of-width scaling to 5 and 4 mm, the caveat under it.
#[test]
fn the_stone_summary_gives_the_size_in_mm_and_the_carat_scaling() {
    let mut design = round_brilliant_design();
    design.girdle_diameter_mm = Some(6.5);
    design.material.name = Some("Diamond".to_string());
    let solved = design.solve().expect("every tier is pinned");
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    // Width x length x total depth: a round stone is as long as it is wide.
    let length = 6.5 * layout.sizes.length_to_width.expect("measured");
    let depth = 6.5 * layout.sizes.height_to_width.expect("measured");
    assert_eq!(format!("{length:.2}"), "6.50");
    let size_row = format!(
        "<tr><th>Size (W \u{d7} L \u{d7} D)</th><td>6.50 \u{d7} 6.50 \u{d7} {depth:.2} mm</td></tr>"
    );
    assert!(html.contains(&size_row), "{size_row}\n{html}");

    // The proportions, as percentages of the width.
    let proportions = design
        .stone_proportions(&solved)
        .expect("the round brilliant closes");
    let table = proportions.table_percent.expect("the fixture has a table");
    let crown = 100.0 * layout.sizes.crown_to_width.expect("measured");
    for row in [
        format!("<tr><th>Table</th><td>{table:.1}% of width</td></tr>"),
        format!("<tr><th>Crown height</th><td>{crown:.1}% of width</td></tr>"),
        "<tr><th>Material</th><td>Diamond</td></tr>".to_string(),
    ] {
        assert!(html.contains(&row), "{row}\n{html}");
    }

    // The carat line: the estimate at 6.50 mm, then at 5 and 4 mm by the cube of the width, two
    // decimals (three under a tenth of a carat), the caveat under it.
    let carat = design
        .yield_report(&solved)
        .carat_weight
        .expect("a diamond with a girdle diameter has a weight");
    let printed = |value: f64| {
        if value < 0.1 {
            format!("{value:.3}")
        } else {
            format!("{value:.2}")
        }
    };
    let weight_at = |mm: f64| carat * (mm / 6.5).powi(3);
    let carat_row = format!(
        "<tr><th>Carat weight (estimate)</th><td>{} ct at 6.50 mm wide; scales with the cube of \
         the width: {} ct at 5 mm, {} ct at 4 mm <span class=\"note\">Assumes the authored \
         facets reach the preform&#39;s own walls.</span></td></tr>",
        printed(carat),
        printed(weight_at(5.0)),
        printed(weight_at(4.0))
    );
    assert!(html.contains(&carat_row), "{carat_row}\n{html}");

    // Nothing random and no date: the same design gives the same page.
    assert_eq!(html, cutting_sheet_html(&design, &solved, None, &[]));
}

/// Without a girdle diameter the summary has no millimetres and no weight, and invents none: it
/// still gives the proportions, the material and the refractive index.
#[test]
fn without_a_girdle_diameter_the_summary_has_no_millimetres() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains("<h3>Stone summary</h3>\n<table class=\"summary\">\n"));
    assert!(!html.contains("<tr><th>Size (W"));
    assert!(!html.contains("<tr><th>Girdle diameter</th>"));
    assert!(!html.contains("Carat weight"));
    assert!(html.contains("<tr><th>Pavilion depth</th><td>"));
    assert!(html.contains("<tr><th>Material</th><td>"));
    assert!(html.contains("<tr><th>Refractive index</th><td>1.540</td></tr>"));
}

/// The carat estimate is scaled to two smaller round widths: about four fifths and three fifths
/// of the width, in whole millimetres (half millimetres under 4 mm), each smaller than the last.
#[test]
fn the_carat_scaling_uses_two_smaller_round_sizes() {
    assert_eq!(smaller_round_sizes(10.0), [8.0, 6.0]);
    assert_eq!(smaller_round_sizes(6.5), [5.0, 4.0]);
    assert_eq!(smaller_round_sizes(12.0), [10.0, 7.0]);
    assert_eq!(smaller_round_sizes(3.5), [3.0, 2.0]);
    assert_eq!(smaller_round_sizes(1.0), [0.5]);
    assert!(smaller_round_sizes(0.5).is_empty());
    assert!(smaller_round_sizes(f64::NAN).is_empty());
}

// ---- Views ---------------------------------------------------------------------------

/// The template's grid holds the four line drawings in the template's order; the shaded
/// renderings are in the additional information, each captioned with its name and direction.
#[test]
fn four_views_are_printed_with_their_labels() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let views: CutViews = render_cut_views(&design, &solved, 120, 100).expect("the stone closes");
    for image in [&views.top, &views.side, &views.end, &views.bottom] {
        assert_eq!((image.width, image.height), (120, 100));
        assert_eq!(&image.png_bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }
    let html = cutting_sheet_html_with(
        &design,
        &solved,
        Some(&views),
        &[],
        &SheetDetails::default(),
    );
    positions_in_order(
        &html,
        &[
            "<div class=\"cell\" title=\"view down +Z\"><svg ",
            "<div class=\"cell\" title=\"view down +X\"><svg ",
            "<div class=\"cell\" title=\"view down -Y\"><svg ",
            "<div class=\"cell\" title=\"view down -Z\"><svg ",
            "<h2 class=\"extra\">Additional information</h2>",
            "<h3>Renderings</h3>\n<div class=\"renders\">",
            "alt=\"Crown, view down +Z\"><figcaption><strong>Crown</strong>, view down +Z</figcaption>",
            "alt=\"Profile, view down +X\"><figcaption><strong>Profile</strong>, view down +X</figcaption>",
            "alt=\"End, view down -Y\"><figcaption><strong>End</strong>, view down -Y</figcaption>",
            "alt=\"Pavilion, view down -Z\"><figcaption><strong>Pavilion</strong>, view down -Z</figcaption>",
            "<h3>Stone summary</h3>",
        ],
    );
    assert_eq!(html.matches("<figure>").count(), 4);
    // The pictures link nowhere: the file stays self-contained.
    assert!(!html.contains("<a "));
    assert_eq!(html.matches("data:image/png;base64,").count(), 4);
    assert!(!html.contains("<p class=\"diagram-missing\">"));
}

/// Without renderings the additional information has none, and the template still has its
/// line drawings.
#[test]
fn without_renderings_the_template_keeps_its_line_drawings() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &SheetDetails::default());
    assert_eq!(html.matches("<figure>").count(), 0);
    assert!(!html.contains("data:image/png;base64,"));
    assert_eq!(html.matches("<div class=\"cell\"").count(), 4);
    assert!(!html.contains("<p class=\"diagram-missing\">"));
}

/// The document builder embeds the four line drawings and the four shaded renderings of a
/// closed design.
#[test]
fn the_document_embeds_the_four_views() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_document(&design, &solved, &[]);
    assert_eq!(html.matches("<figure>").count(), 4);
    assert_eq!(html.matches("<div class=\"cell\"").count(), 4);
}

/// The line drawings are lines and labels: every view has its outline and its edges, the crown
/// and pavilion their wheel, and the SVG names the tier codes on the facets.
#[test]
fn the_line_drawings_carry_lines_and_tier_codes() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let views = render_vector_cut_views(&design, &solved, VECTOR_VIEW_WIDTH, VECTOR_VIEW_HEIGHT)
        .expect("the stone closes");
    let captions: Vec<&str> = views.iter().map(|view| view.caption.as_str()).collect();
    assert_eq!(captions, ["CROWN", "PROFILE", "END", "PAVILION"]);
    for view in &views {
        let count = |weight: LineWeight| {
            view.lines
                .iter()
                .filter(|line| line.weight == weight)
                .count()
        };
        assert!(count(LineWeight::Outline) > 0, "{}", view.caption);
        assert!(count(LineWeight::Edge) > 0, "{}", view.caption);
    }
    let [crown, _, _, pavilion] = &views;
    assert_eq!(
        crown
            .lines
            .iter()
            .filter(|line| line.weight == LineWeight::Tick)
            .count(),
        96,
        "one tick per tooth"
    );
    // Codes alone, not `C1 0`: the crown carries the table and its crown tiers.
    let crown_labels: Vec<&str> = crown
        .labels
        .iter()
        .map(|label| label.text.as_str())
        .collect();
    assert!(crown_labels.contains(&"T"), "{crown_labels:?}");
    assert!(
        crown_labels.iter().any(|label| label.starts_with('C')),
        "{crown_labels:?}"
    );
    assert!(crown_labels.contains(&"96"), "tooth 0 is the tooth count");
    assert!(!crown_labels.contains(&"0"), "{crown_labels:?}");
    assert!(!crown_labels.iter().any(|label| label.contains(' ')));
    let pavilion_svg = view_to_svg(pavilion);
    assert!(pavilion_svg.contains("<path d=\"M"), "{pavilion_svg}");
    assert!(
        pavilion_svg.contains(">P"),
        "a pavilion code: {pavilion_svg}"
    );

    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(html.contains(">T</text>"));
    assert!(html.contains(">CROWN</text>"));
}

// ---- SheetDetails --------------------------------------------------------------------

/// `cutting_sheet_document` is `cutting_sheet_document_with` the design's own details, so the
/// web gets the new layout with the fallback header.
#[test]
fn the_plain_document_equals_the_document_with_the_fallback_details() {
    let mut design = round_brilliant_design();
    design.meta.headers = vec!["Fallback Stone".to_string(), "by Someone".to_string()];
    let solved = design.solve().expect("every tier is pinned");
    assert_eq!(
        cutting_sheet_document(&design, &solved, &[]),
        cutting_sheet_document_with(&design, &solved, &[], &SheetDetails::from_design(&design))
    );
}

/// The fallback reads the `.asc` header and footnote lines and invents nothing.
#[test]
fn from_design_reads_the_asc_header_and_footnotes() {
    let mut design = round_brilliant_design();
    design.meta.headers = vec![
        "  ".to_string(),
        "Star Burst".to_string(),
        "BY Jane Doe".to_string(),
        "A third line".to_string(),
    ];
    design.meta.footnotes = vec![
        String::new(),
        "  Cut dry. ".to_string(),
        "Mind the table.".to_string(),
    ];
    let details = SheetDetails::from_design(&design);
    assert_eq!(details.title, "Star Burst");
    assert_eq!(details.author, "Jane Doe");
    assert_eq!(details.comments, ["Cut dry.", "Mind the table."]);
    assert_eq!(details.subtitle, "");
    assert_eq!(details.date_text, "");
    assert_eq!(details.shape, "");
    assert_eq!(details.ri_range, None);
    assert_eq!(details.size_range_mm, None);
}

/// A design with no header lines gets an entirely empty fallback; a header that is only a
/// "by" line gives an author and no title.
#[test]
fn from_design_invents_nothing() {
    let design = round_brilliant_design();
    assert_eq!(SheetDetails::from_design(&design), SheetDetails::default());

    let mut by_only = round_brilliant_design();
    by_only.meta.headers = vec!["by Jane".to_string()];
    let details = SheetDetails::from_design(&by_only);
    assert_eq!(details.title, "");
    assert_eq!(details.author, "Jane");
}

/// The native file's title, designer and shape replace the fallback when not empty, its notes
/// are appended to the comments one line each, and the date is the caller's.
#[test]
fn from_metadata_layers_the_file_details_over_the_fallback() {
    let mut design = round_brilliant_design();
    design.meta.headers = vec!["Header Title".to_string(), "by Header Author".to_string()];
    design.meta.footnotes = vec!["From the footnote".to_string()];
    let metadata = DesignMetadata {
        title: " File Title ".to_string(),
        designer: "Capps, Jerry".to_string(),
        shape: "Round".to_string(),
        notes: "First note\n\n  Second note  \n".to_string(),
        ..DesignMetadata::default()
    };
    let details = SheetDetails::from_metadata(&design, &metadata, "October 2026");
    assert_eq!(details.title, "File Title");
    assert_eq!(details.author, "Capps, Jerry");
    assert_eq!(details.shape, "Round");
    assert_eq!(
        details.comments,
        ["From the footnote", "First note", "Second note"]
    );
    assert_eq!(details.date_text, "October 2026");

    // Empty file fields fall back to the header lines.
    let bare = SheetDetails::from_metadata(&design, &DesignMetadata::default(), "");
    assert_eq!(bare.title, "Header Title");
    assert_eq!(bare.author, "Header Author");
    assert_eq!(bare.shape, "");
    assert_eq!(bare.comments, ["From the footnote"]);
    assert_eq!(bare.date_text, "");
}

/// The date text is built from numbers the caller reads off its own clock.
#[test]
fn month_year_text_is_english() {
    assert_eq!(month_year_text(2026, 10), "October 2026");
    assert_eq!(month_year_text(1999, 1), "January 1999");
    assert_eq!(month_year_text(2026, 0), "");
    assert_eq!(month_year_text(2026, 13), "");
}

// ---- Text sheet ----------------------------------------------------------------------

/// The text sheet carries the same counts and rows as the HTML, and its index lists print
/// index 0 as the gear's tooth count as the template does.
#[test]
fn the_text_sheet_mirrors_the_html_blocks() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Plain".to_string(),
        author: "A. Cutter".to_string(),
        date_text: "October 2026".to_string(),
        ..SheetDetails::default()
    };
    let text = cutting_sheet_text_with(&design, &solved, &[], &details);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("Plain"));
    assert_eq!(lines.next(), Some("by A. Cutter"));
    assert_eq!(lines.next(), Some("October 2026"));
    assert!(text.contains("Crown     tiers 3+1   facets 32+1"), "{text}");
    assert!(text.contains("Total     tiers 8     facets 74"), "{text}");
    assert!(text.contains("L/W 1.000"), "{text}");
    assert!(text.contains("  RI range: 1.540"), "{text}");
    assert!(text.contains("  Symmetry: 8-fold, mirror\n"), "{text}");
    assert!(text.contains("  Index gear: 96 Index\n"), "{text}");
    assert!(text.contains("indices [96-12-24-36-48-60-72-84]"), "{text}");
    assert!(!text.contains("[00-"), "{text}");
    // One row line per tier, in the two sections.
    assert_eq!(text.matches(" angle ").count(), design.tiers.len());
    // The rows are the codes of the HTML, in the same order.
    let html = cutting_sheet_html_with(&design, &solved, None, &[], &details);
    let mut from = 0;
    for label in label_cells(&html) {
        let at = text[from..]
            .find(&format!(". {label} "))
            .unwrap_or_else(|| panic!("{label} out of order in the text sheet"));
        from += at + 1;
    }
    assert_eq!(section_html(&html, "Pavilion").matches("<tr").count(), 4);
}

/// A custom material whose stored default is "Set, tilt 0" has the same baked axes as a plain
/// one, so only the stored default (passed by the caller) makes the sheet print the block; with
/// `None` the sheet is exactly the one `SheetLayout::build` gives.
#[test]
fn a_stored_default_of_tilt_zero_prints_the_orientation_block_and_none_changes_nothing() {
    use pleochroma::optics::materials::GemMaterial;
    use pleochroma_cut_core::CrystalOrientation;
    let mut material = GemMaterial::sapphire();
    material.name = "Blue Fan".to_owned();
    let mut design = round_brilliant_design();
    design.material.name = Some("Blue Fan".to_owned());
    let solved = design.solve().expect("every tier is pinned");
    let catalogue = [material];
    let details = SheetDetails::default();

    let plain = SheetLayout::build(&design, &solved, &catalogue, &details);
    assert!(plain.sheet.orientation.is_none());
    let none = SheetLayout::build_with_default(&design, &solved, &catalogue, None, &details);
    assert_eq!(none.sheet.header, plain.sheet.header);

    let stored = CrystalOrientation::from_angles(0.0, 0.0, None);
    let with =
        SheetLayout::build_with_default(&design, &solved, &catalogue, Some(stored), &details);
    let block = with
        .sheet
        .orientation
        .expect("a stored default is an axis that is set");
    assert_eq!(block.orientation, stored);
    assert!(with.sheet.header.len() > plain.sheet.header.len());
}
