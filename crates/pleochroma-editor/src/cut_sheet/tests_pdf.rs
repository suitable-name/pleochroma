//! Tests for the PDF sheet ([`super::pdf`]): the file's shape, its text, its pages and its
//! determinism, on the same fixtures as the HTML sheet.

use super::{
    SheetDetails, SheetLayout, VECTOR_VIEW_HEIGHT, VECTOR_VIEW_WIDTH, cutting_sheet_pdf,
    cutting_sheet_pdf_with, pdf::sheet_pages, render_vector_cut_views,
    tests::round_brilliant_design,
};
use pleochroma_cut_core::Design;

/// Whether `bytes` contains `needle`.
fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

/// The round brilliant is one PDF page: a PDF file, one page in its tree, the title and both
/// section headings as text, the two standard fonts by name and nothing embedded.
#[test]
fn the_round_brilliant_is_a_one_page_pdf_with_its_title_and_sections() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let details = SheetDetails {
        title: "Round Test".to_string(),
        ..SheetDetails::default()
    };
    let pdf = cutting_sheet_pdf_with(&design, &solved, &[], &details).expect("the stone closes");
    assert!(pdf.starts_with(b"%PDF-"));
    assert!(contains(&pdf, b"%%EOF"));
    assert!(contains(&pdf, b"/Count 1"), "one page");
    assert!(contains(&pdf, b"(Round Test)"), "the title");
    assert!(contains(&pdf, b"(Pavilion)"));
    assert!(contains(&pdf, b"(Crown)"));
    assert!(contains(&pdf, b"(Facet Data)"));
    assert!(contains(&pdf, b"/Helvetica-Bold"));
    assert!(contains(&pdf, b"/WinAnsiEncoding"));
    assert!(!contains(&pdf, b"/FontFile"), "no embedded font");
    assert!(!contains(&pdf, b"continued"), "no continuation page");

    let layout = SheetLayout::build(&design, &solved, &[], &details);
    let views = render_vector_cut_views(&design, &solved, VECTOR_VIEW_WIDTH, VECTOR_VIEW_HEIGHT)
        .expect("the stone closes");
    assert_eq!(sheet_pages(&layout, &views).len(), 1);
}

/// The index lists print index 0 as the gear's tooth count in the PDF too, and the angle and
/// code columns are there as text.
#[test]
fn the_pdf_rows_carry_the_template_columns() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let pdf = cutting_sheet_pdf(&design, &solved, &[]).expect("the stone closes");
    assert!(contains(&pdf, b"(96-12-24-36-48-60-72-84)"));
    assert!(!contains(&pdf, b"(00-12-24"));
    assert!(contains(&pdf, b"(Table)"));
    assert!(contains(&pdf, b"(Culet)"));
    // The degree sign is outside ASCII, so a string holding it is written in hex: `41.00°` is
    // 34 31 2E 30 30 B0.
    assert!(contains(&pdf, b"<34312E3030B0>") || contains(&pdf, b"<34312e3030b0>"));
}

/// The same design gives the same bytes: no date, no document ID.
#[test]
fn the_pdf_is_deterministic() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let first = cutting_sheet_pdf(&design, &solved, &[]).expect("the stone closes");
    let second = cutting_sheet_pdf(&design, &solved, &[]).expect("the stone closes");
    assert_eq!(first, second);
    assert!(!contains(&first, b"/CreationDate"));
    assert!(!contains(&first, b"/ID"));
}

/// A long comment block pushes the sheet onto a second page; the template still draws both
/// sections.
#[test]
fn a_long_sheet_runs_onto_further_pages() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let views = render_vector_cut_views(&design, &solved, VECTOR_VIEW_WIDTH, VECTOR_VIEW_HEIGHT)
        .expect("the stone closes");
    let mut layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
    // Sixty comment lines push the sections down past the first page.
    layout.details.comments = (0..60).map(|i| format!("Comment line {i}.")).collect();
    let pages = sheet_pages(&layout, &views);
    assert!(pages.len() > 1, "{} pages", pages.len());
    let bytes: Vec<u8> = pages
        .into_iter()
        .flat_map(pdf_writer::Content::finish)
        .collect();
    assert!(contains(&bytes, b"(Comment line 59.)"));
    assert!(contains(&bytes, b"(Pavilion"));
    assert!(contains(&bytes, b"(Crown"));
}

/// A concave design prints its tool lines in the template's columns.
#[test]
fn a_concave_design_prints_its_tool_lines() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let pdf = cutting_sheet_pdf(&design, &solved, &[]).expect("the concave fixture closes");
    for tier in &design.concave_tiers {
        let [code, ..] = tier.second_line_fields();
        let needle = format!("({code})");
        assert!(contains(&pdf, needle.as_bytes()), "{code}");
    }
}
