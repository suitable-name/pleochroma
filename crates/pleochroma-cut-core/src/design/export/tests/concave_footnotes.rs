use super::*;

/// Editing a tier after an export must not leave the old export's lines beside the
/// new ones: the reimported footnotes are recognised by their shape and replaced,
/// leaving exactly two lines per tier and the user's own notes.
#[test]
fn an_edited_concave_tier_leaves_no_stale_footnotes_on_the_next_export() {
    let mut design = round_brilliant_design();
    design.meta.footnotes = vec!["my own note".to_string()];
    concave_pair(&mut design);
    let solved = design.solve().expect("solves");
    let first = design.to_asc_schedule_for_export(&solved);
    let text = pleochroma_formats::asc::to_asc_string(&first).expect("writes");

    let parsed = pleochroma_formats::asc::parse_asc(&text).expect("parses");
    let mut reimported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    concave_pair(&mut reimported);
    // Edit Z of the first tier, its instructions and the second tier's tool.
    reimported.concave_tiers[0].displacement[2] = 0.2;
    reimported.concave_tiers[0].instructions = "cut deeper".to_string();
    reimported.concave_tiers[1].diameter_ratio = 0.45;
    let solved = reimported.solve().expect("solves");
    let second = reimported.to_asc_schedule_for_export(&solved);

    assert_eq!(
        second.footnotes,
        vec![
            "my own note".to_string(),
            "Groove  -62.00  3 11.50 19  cut deeper  [concave tier]".to_string(),
            "CYL  +90.00 deg  X = 0.000, Y = 0.120, Z = 0.200  D/W = 0.250, reciprocating  \
             [concave tier]"
                .to_string(),
            "Pit  40.00  0  [concave tier]".to_string(),
            "CON  0.00 deg  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.450, angle = 60.00 deg, \
             plunge  [concave tier]"
                .to_string(),
        ]
    );
    // A third round is stable.
    let text = pleochroma_formats::asc::to_asc_string(&second).expect("writes");
    let parsed = pleochroma_formats::asc::parse_asc(&text).expect("parses");
    let mut again = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    again.concave_tiers = reimported.concave_tiers.clone();
    again.concave_tier_ids = reimported.concave_tier_ids.clone();
    let solved = again.solve().expect("solves");
    assert_eq!(
        again.to_asc_schedule_for_export(&solved).footnotes,
        second.footnotes
    );
}

/// Only a line the export marked (or an unmarked line an earlier version wrote for a tier
/// the design has now, byte for byte) is generated; everything else is the user's, however
/// it looks.
#[test]
fn only_marked_or_legacy_lines_of_current_tiers_are_stripped() {
    let mut design = round_brilliant_design();
    concave_pair(&mut design);
    let mut notes: Vec<String> = [
        "Cut slowly, 45.00 rpm",
        // Marked: the lines of an export, for these tiers or an earlier shape of them.
        "Groove  -62.00  3 11.50 19  cut to depth  [concave tier]",
        "CYL  +90.00 deg  X = 0.000, Y = 0.500, Z = 0.050  D/W = 0.250, reciprocating  \
         [concave tier]",
        // Unmarked, written by an earlier version for the Groove tier as it is now.
        "Groove  -62.00  3 11.50 19  cut to depth",
        "CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating",
        // The user's: shaped like a tool line, but for a tool no tier has.
        "DSC  -3.50°  X = 1.000, Y = -0.500, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge",
        "Notch  12.00  5  by hand",
        "SPH note: CYL  not a tool line",
        // The marker must be a field of its own.
        "tagged[concave tier]",
        "keep me",
    ]
    .map(String::from)
    .to_vec();
    super::super::schedule::strip_generated_concave_footnotes(&mut notes, &design.concave_tiers);
    assert_eq!(
        notes,
        vec![
            "Cut slowly, 45.00 rpm".to_string(),
            "DSC  -3.50°  X = 1.000, Y = -0.500, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge"
                .to_string(),
            "Notch  12.00  5  by hand".to_string(),
            "SPH note: CYL  not a tool line".to_string(),
            "tagged[concave tier]".to_string(),
            "keep me".to_string(),
        ]
    );
}

/// A note the user typed in the shape of a facet line and a tool line (copied from a
/// sheet, say) is not the export's: it survives the paired load's strip and the next export,
/// and the export's own lines still replace each other.
#[test]
fn a_users_own_tool_shaped_footnotes_survive_the_export_round_trip() {
    let users_pair = [
        "Notch  12.00  5  by hand".to_string(),
        "CYL  +45.00°  X = 0.100, Y = 0.200, Z = 0.300  D/W = 0.300, plunge".to_string(),
    ];
    let mut design = round_brilliant_design();
    design.meta.footnotes = users_pair.to_vec();
    concave_pair(&mut design);
    let solved = design.solve().expect("solves");
    let first = design.to_asc_schedule_for_export(&solved);
    assert_eq!(
        first.footnotes[..2],
        users_pair,
        "the export keeps the user's lines first"
    );
    assert_eq!(first.footnotes.len(), 6);

    // What a paired load does to the footnotes the `.asc` carries.
    let text = pleochroma_formats::asc::to_asc_string(&first).expect("writes");
    let parsed = pleochroma_formats::asc::parse_asc(&text).expect("parses");
    let mut loaded = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &parsed);
    concave_pair(&mut loaded);
    super::super::schedule::strip_generated_concave_footnotes(
        &mut loaded.meta.footnotes,
        &loaded.concave_tiers,
    );
    assert_eq!(loaded.meta.footnotes, users_pair);

    let solved = loaded.solve().expect("solves");
    assert_eq!(
        loaded.to_asc_schedule_for_export(&solved).footnotes,
        first.footnotes
    );
}

/// The two footnotes per tier that `concave_pair`'s tiers got in an `.asc` written before the
/// export marked its lines, as a load reads them back (the `.asc` parser trims every
/// footnote line, so the empty instructions of the second tier leave no trailing spaces).
///
/// Written out by hand from what that build produced (the facet line, then the tool line
/// with `°`), NOT from the current formulas: a change to a formula must not quietly change
/// what counts as an earlier file's line.
const EARLIER_VERSION_FOOTNOTES: [&str; 4] = [
    "Groove  -62.00  3 11.50 19  cut to depth",
    "CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating",
    "Pit  40.00  0",
    "CON  0.00°  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.500, angle = 60.00°, plunge",
];

/// A file an earlier version exported carries unmarked lines with `°`; the paired load
/// still recognises those of the tiers it restores, so the next export does not write each
/// line twice.
#[test]
fn footnotes_an_earlier_version_wrote_are_replaced_not_duplicated() {
    let mut design = round_brilliant_design();
    design.meta.footnotes = vec!["my own note".to_string()];
    concave_pair(&mut design);
    assert!(
        EARLIER_VERSION_FOOTNOTES
            .iter()
            .any(|line| line.contains('°'))
    );
    let mut loaded = design.clone();
    loaded.meta.footnotes = std::iter::once("my own note".to_string())
        .chain(EARLIER_VERSION_FOOTNOTES.map(String::from))
        .collect();

    super::super::schedule::strip_generated_concave_footnotes(
        &mut loaded.meta.footnotes,
        &loaded.concave_tiers,
    );
    assert_eq!(loaded.meta.footnotes, vec!["my own note".to_string()]);
    let solved = loaded.solve().expect("solves");
    assert_eq!(
        loaded.to_asc_schedule_for_export(&solved).footnotes,
        design.to_asc_schedule_for_export(&solved).footnotes
    );
}
