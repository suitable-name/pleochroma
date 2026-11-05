//! Tests for [`super::diagram`]/[`super::html`]: pure-function coverage with a
//! plain `Design` fixture and no window, per this group's own module doc comment.

use super::{
    SheetDetails, SheetLayout,
    diagram::render_cut_diagram,
    html::{base64_encode, cutting_sheet_html, html_escape, views_block},
    layout::{material_label, template_instruction},
};
use pleochroma::{
    geometry::meet_solver::{Block, classify_blocks},
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{
    ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta, design::TierRef,
};

/// The code cell of every facet line of the sheet's section tables (the Pavilion section, then
/// the Crown section), in printed order. A concave tier's tool line (`class="... tool"`) is not
/// a facet line.
pub(super) fn label_cells(html: &str) -> Vec<String> {
    html.split("<table class=\"rows\">")
        .skip(1)
        .flat_map(|table| {
            let table = table.split("</table>").next().unwrap_or(table);
            table.split("<tr").skip(1).filter_map(|row| {
                let (opening, rest) = row.split_once('>')?;
                if opening.contains("tool") {
                    return None;
                }
                let code = rest.split("<td class=\"code\">").nth(1)?;
                Some(code.split("</td>").next()?.to_string())
            })
        })
        .collect()
}

/// The HTML of the section headed `heading` (`Pavilion` or `Crown`): from its `<section>` tag
/// to the closing `</section>`.
pub(super) fn section_html<'a>(html: &'a str, heading: &str) -> &'a str {
    let opener = format!("<section class=\"cut-section\">\n<h2 class=\"section\">{heading}</h2>");
    let start = html
        .find(&opener)
        .unwrap_or_else(|| panic!("no {heading} section in the sheet"));
    let rest = &html[start..];
    &rest[..rest.find("</section>").expect("the section closes")]
}

/// The template's code cell holding `code`.
pub(super) fn code_cell(code: &str) -> String {
    format!("<td class=\"code\">{code}</td>")
}

/// The template's instruction cell holding `text`.
fn instruction_cell(text: &str) -> String {
    format!("<td class=\"instruction\">{}</td>", html_escape(text))
}

/// The same round-brilliant fixture `pleochroma_cut_core::cutting_sheet`'s
/// own tests use: 8 tiers, all `ScaleReference`, always solves and closes --
/// see that crate's `design/tier.rs::standard_round_brilliant_template_solves_and_closes`.
pub(super) fn round_brilliant_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn custom_garnet(n_d: f32) -> GemMaterial {
    let mut gem = GemMaterial::diamond();
    gem.name = "My Garnet".to_string();
    gem.dispersion = pleochroma::optics::dispersion::DispersionModel::Cauchy {
        a: n_d,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// A design on a CUSTOM catalogue material must print that material's own
/// `n_D` in the printed sheet's "RI range" row when `custom` is
/// supplied -- the bug this module fixes. `&[]` (no catalogue) must still fall
/// back to the legacy schedule RI, matching the built-ins-only behaviour every
/// other test in this module already relies on.
#[test]
fn cutting_sheet_html_prints_a_custom_materials_own_refractive_index() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    let solved = design.solve().expect("every tier is pinned");
    let custom = [custom_garnet(1.9)];

    let with_catalogue = cutting_sheet_html(&design, &solved, None, &custom);
    assert!(
        with_catalogue.contains("<td>1.900 (My Garnet)</td>"),
        "{with_catalogue}"
    );

    let without_catalogue = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(
        without_catalogue.contains(&format!(
            "<td>{:.3} (My Garnet)</td>",
            design.meta.refractive_index
        )),
        "{without_catalogue}"
    );
}

/// An unnamed design (every untouched import) whose effective RI sits at
/// sapphire's own must print the guess "Sapphire?", never the bare name passed
/// off as fact.
#[test]
fn material_label_shows_a_guess_for_an_unnamed_design_near_a_builtin() {
    let mut design = round_brilliant_design();
    design.meta.refractive_index = 1.76;
    design.material = MaterialSelection::none();
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_label(&design, n_d).as_deref(), Some("Sapphire?"));
}

/// Once a material name IS set, the line prints it plainly -- no
/// guess wording once something is a stated fact.
#[test]
fn material_label_prints_the_name_plainly_once_set() {
    let mut design = round_brilliant_design();
    design.material = MaterialSelection {
        name: Some("Sapphire".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_label(&design, n_d).as_deref(), Some("Sapphire"));
}

/// An unnamed design whose index is close to no built-in prints no material at all: the line
/// is the bare index, not an invented species.
#[test]
fn material_label_is_absent_when_nothing_is_close() {
    let mut design = round_brilliant_design();
    design.meta.refractive_index = 1.90;
    design.material = MaterialSelection::none();
    let n_d = design.effective_refractive_index_with(&[]);
    assert_eq!(material_label(&design, n_d), None);
}

/// The generated sheet must carry one line per tier, in cutting order, with the code, the
/// instruction and (in the mast table) the tier's name and formatted mast all present -- a
/// basic "does the known schedule show up" check before the more targeted tests below.
#[test]
fn cutting_sheet_html_contains_expected_rows() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::from_design(&design));

    // No title on the fixture: the heading is "Cutting instructions".
    assert!(html.contains("<h1 class=\"title\">Cutting instructions</h1>"));
    assert!(html.contains(&code_cell("T")), "the table's code is T");
    assert!(
        html.contains("<td class=\"code\">T</td><td class=\"angle\">0.00\u{b0}</td>"),
        "{html}"
    );
    assert!(
        html.contains("<td class=\"indices\">Table</td>"),
        "the table's index column reads Table"
    );
    assert!(html.contains(&code_cell("Culet")));
    // Computed meets (cut-core `cutting_sheet`): the girdle sets the stone size, later tiers
    // meet the earlier-cut tiers they touch, the pavilion mains touch none and print their mast.
    // The template prints the instruction without the tier's name in front.
    for instruction in [
        "Set stone size.",
        "Meet C1, C2",
        "Meet C1",
        "Set to mast depth 0.6700",
    ] {
        assert!(
            html.contains(&format!("<td class=\"instruction\">{instruction}")),
            "{instruction}"
        );
    }
    for row in &layout.sheet.rows {
        let instruction = template_instruction(row);
        assert!(
            html.contains(&instruction_cell(&instruction)),
            "{instruction}"
        );
        // The mast table carries the mast.
        assert!(
            html.contains(&format!("<td>{:.4}</td>", row.mast)),
            "{}",
            row.mast
        );
    }
    assert!(
        html.contains("<td>Pavilion Main</td>"),
        "the name in the mast table"
    );
    // One line and one `<tbody>` per tier inside the two section tables.
    assert_eq!(label_cells(&html).len(), design.tiers.len());
    let bodies: usize = html
        .split("<table class=\"rows\">")
        .skip(1)
        .map(|table| {
            table
                .split("</table>")
                .next()
                .unwrap_or("")
                .matches("<tbody>")
                .count()
        })
        .sum();
    assert_eq!(bodies, design.tiers.len());
}

/// Every row must print a positive angle (the unsigned magnitude of its angle)
/// in accordance with international faceting standards.
#[test]
fn angle_column_prints_the_positive_angle_magnitude() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    // "Pavilion Main" is authored at -41.0 degrees; its angle cell must
    // print the positive magnitude (41.00°), not a negative angle.
    assert!(
        html.contains("<td class=\"angle\">41.00\u{b0}</td>"),
        "{html}"
    );
    assert!(!html.contains("-41.00"), "{html}");
}

/// A carat-weight row must appear only once a girdle diameter anchors a real
/// mm scale -- matching the mm column's own gating.
#[test]
fn carat_weight_row_appears_only_when_girdle_diameter_is_set() {
    let design_no_mm = round_brilliant_design();
    let solved = design_no_mm.solve().expect("every tier is pinned");
    let html_no_mm = cutting_sheet_html(&design_no_mm, &solved, None, &[]);
    assert!(!html_no_mm.contains("Carat weight"));

    let mut design_with_mm = round_brilliant_design();
    design_with_mm.girdle_diameter_mm = Some(6.5);
    // A carat weight needs a specific gravity too, so a material must be
    // named -- the same fixture cut-core's own header test uses.
    design_with_mm.material.name = Some("Diamond".to_string());
    let html_with_mm = cutting_sheet_html(&design_with_mm, &solved, None, &[]);
    assert!(
        html_with_mm.contains("Carat weight (estimate)"),
        "{html_with_mm}"
    );
}

/// The "Mast (mm)" column -- and any mm-formatted cell -- must appear only
/// when `girdle_diameter_mm` is set, never derived from whether the mm
/// conversion happens to resolve.
#[test]
fn mm_column_appears_only_when_girdle_diameter_is_set() {
    let design_no_mm = round_brilliant_design();
    let solved = design_no_mm.solve().expect("every tier is pinned");
    let html_no_mm = cutting_sheet_html(&design_no_mm, &solved, None, &[]);
    assert!(!html_no_mm.contains("Mast (mm)"));
    assert!(
        !html_no_mm.contains("<tr><th>Size (W"),
        "no millimetre size line"
    );

    let mut design_with_mm = round_brilliant_design();
    design_with_mm.girdle_diameter_mm = Some(6.5);
    let solved_mm = design_with_mm.solve().expect("every tier is pinned");
    let html_with_mm = cutting_sheet_html(&design_with_mm, &solved_mm, None, &[]);
    assert!(html_with_mm.contains("Mast (mm)"));
    assert!(
        html_with_mm.contains("<tr><th>Size (W \u{d7} L \u{d7} D)</th><td>6.50 \u{d7} "),
        "the Stone summary's size line starts at the girdle diameter: {html_with_mm}"
    );
}

/// The section a row is in must come from the solver's own `classify_blocks`, not the sign
/// of the printed angle -- the mistake this codebase already made once.
/// `standard_round_brilliant`'s own tiers give a known split: Table/Star/Crown Main/Upper
/// Girdle are Crown, the 90-degree `Girdle` tier is Girdle (printed in the Pavilion
/// section), and Pavilion Main/Lower Girdle/Culet (the last at a sign-negative zero angle,
/// which is the pavilion side) are Pavilion.
#[test]
fn sections_match_classify_blocks_not_angle_sign() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let blocks = classify_blocks(&design.meet_tier_inputs());
    assert_eq!(
        blocks,
        vec![
            Block::Crown,
            Block::Crown,
            Block::Crown,
            Block::Crown,
            Block::Girdle,
            Block::Pavilion,
            Block::Pavilion,
            Block::Pavilion,
        ]
    );

    let html = cutting_sheet_html(&design, &solved, None, &[]);
    // "Culet" sits at angle `-0.0` -- a naive sign-of-the-formatted-string
    // check would call that Crown (`-0.0` formats with no visible sign in
    // some locales, or as positive zero); `classify_blocks` reads the sign of
    // the zero and puts it on the pavilion, and that is the section it must print in.
    let pavilion = section_html(&html, "Pavilion");
    let crown = section_html(&html, "Crown");
    assert!(
        pavilion.contains(&code_cell("Culet")),
        "Culet belongs to the Pavilion section: {pavilion}"
    );
    assert!(!crown.contains(&code_cell("Culet")), "{crown}");
    assert!(
        pavilion.contains(&code_cell("G1")),
        "the girdle is part of the Pavilion section: {pavilion}"
    );
    assert!(!crown.contains(&code_cell("G1")), "{crown}");
}

/// `render_cut_diagram` must produce a non-empty PNG for a design that
/// closes, sized exactly as requested.
#[test]
fn render_cut_diagram_produces_a_sized_png_for_a_closed_design() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let image = render_cut_diagram(&design, &solved, 200, 100)
        .expect("standard round brilliant closes to a solid");
    assert_eq!(image.width, 200);
    assert_eq!(image.height, 100);
    assert_ne!(image.png_bytes, [] as [u8; 0]);
    // PNG magic bytes.
    assert_eq!(&image.png_bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
}

/// The shaded picture is embedded in the additional information only when one is supplied;
/// the template's line drawings are there either way, and the "unavailable" notice replaces
/// them only for a design without drawings.
#[test]
fn diagram_embedding_switches_on_the_option() {
    // The ELEMENT, not the bare class name: `.diagram-missing` is also a rule in
    // the sheet's own stylesheet.
    const NOTICE: &str = "<p class=\"diagram-missing\">";

    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let image = render_cut_diagram(&design, &solved, 64, 32).expect("closes to a solid");

    let with_image = cutting_sheet_html(&design, &solved, Some(&image), &[]);
    assert!(with_image.contains("data:image/png;base64,"));
    assert!(
        with_image.contains("<figure class=\"wide\"><img "),
        "the single picture spans the renderings grid"
    );
    assert!(with_image.contains("<figcaption>2D faceting diagram</figcaption>"));
    assert!(!with_image.contains(NOTICE));
    assert_eq!(with_image.matches("<svg ").count(), 4, "the line drawings");

    let without_image = cutting_sheet_html(&design, &solved, None, &[]);
    assert!(!without_image.contains("data:image/png;base64,"));
    assert!(!without_image.contains("<h3>Renderings</h3>"));
    assert_eq!(without_image.matches("<svg ").count(), 4);

    let missing = views_block(None);
    assert!(missing.contains(NOTICE), "{missing}");
    assert!(!missing.contains("<svg"));
}

/// The hand-rolled base64 encoder must match RFC 4648 on values covering
/// both padding cases (`chunks(3)` remainders of 1 and 2 bytes) and the
/// empty input.
#[test]
fn base64_encode_matches_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

/// HTML-unsafe characters in a tier name must be escaped, not interpolated
/// raw -- a `.asc` file's own free-text header/notes can legally contain
/// `<`/`&`/quotes.
#[test]
fn html_escape_handles_unsafe_characters() {
    assert_eq!(
        html_escape("A & B <tag> \"quoted\" 'apos'"),
        "A &amp; B &lt;tag&gt; &quot;quoted&quot; &#39;apos&#39;"
    );
}

/// Each concave tier's facet line and tool line sit in ONE `<tbody>` on ONE band (so the band
/// and the page-break rule cover the pair), every tier gets its own `<tbody>`, and the tool
/// line carries the very strings `second_line_fields` gives, in the template's columns.
#[test]
fn html_sheet_groups_each_concave_pair_in_one_tbody() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    let bodies: Vec<&str> = html
        .split("<tbody>")
        .skip(1)
        .map(|body| body.split("</tbody>").next().unwrap_or(body))
        .collect();
    let tiers = design.tiers.len() + design.concave_tiers.len();
    // The mast table of the additional information has one more `<tbody>`.
    assert_eq!(
        bodies.len(),
        tiers + 1,
        "one tbody per tier, flat and concave"
    );

    let with_tool: Vec<&&str> = bodies
        .iter()
        .filter(|body| body.contains("tool\""))
        .collect();
    assert_eq!(with_tool.len(), design.concave_tiers.len());
    for body in with_tool {
        assert_eq!(body.matches("<tr").count(), 2, "facet line and tool line");
        // Both lines share the band: both shaded or neither.
        assert!(
            body.matches("shaded").count() % 2 == 0,
            "one band per tier: {body}"
        );
    }
    for tier in &design.concave_tiers {
        let [code, theta, displacement, details] = tier.second_line_fields();
        assert!(html.contains(&code_cell(&html_escape(&code))), "{code}");
        assert!(
            html.contains(&format!("<td class=\"angle\">{}</td>", html_escape(&theta))),
            "{theta}"
        );
        assert!(
            html.contains(&format!(
                "<td class=\"indices\">{}</td>",
                html_escape(&displacement)
            )),
            "{displacement}"
        );
        assert!(html.contains(&instruction_cell(&details)), "{details}");
    }
    // The fixture sets a girdle diameter, so the mast table gives each tool in millimetres.
    assert!(html.contains("<th>Tool (mm)</th>"));
}

/// The lines alternate bands per tier, the first tier of a section shaded.
#[test]
fn html_sections_band_every_other_tier_starting_shaded() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    for heading in ["Pavilion", "Crown"] {
        let section = section_html(&html, heading);
        let rows: Vec<&str> = section.split("<tr").skip(1).collect();
        assert_eq!(rows.len(), 4, "{heading}");
        for (position, row) in rows.iter().enumerate() {
            assert_eq!(
                row.starts_with(" class=\"shaded\""),
                position % 2 == 0,
                "{heading} row {position}: {row}"
            );
        }
    }
}

/// A planar design is no longer printed in its stored order: the fixture is stored top-down
/// (table first), the sheet lists the pavilion and girdle tiers first (the Pavilion
/// section), the crown next and the table last (the Crown section), each line labelled with
/// its code.
#[test]
fn html_rows_of_a_planar_design_follow_the_cutting_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    assert_eq!(
        label_cells(&html),
        ["G1", "P1", "P2", "Culet", "C1", "C2", "C3", "T"]
    );
    assert_eq!(
        label_cells(section_html(&html, "Pavilion")),
        ["G1", "P1", "P2", "Culet"],
        "the girdle is cut with the pavilion"
    );
    let crown = section_html(&html, "Crown");
    assert_eq!(label_cells(crown), ["C1", "C2", "C3", "T"]);
    // The template has no name column: the tier's name is in the mast table only.
    assert!(!section_html(&html, "Pavilion").contains("Pavilion Main"));
}

/// The stored order still decides the order inside a section: the fixture stored bottom-up
/// (culet first, table last) prints the pavilion and girdle tiers in that stored order, then
/// the crown tiers in theirs, the table last, and the codes are numbered along the rows.
#[test]
fn html_rows_keep_the_stored_order_inside_a_section() {
    let mut design = round_brilliant_design();
    design.tiers.reverse();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert_eq!(
        label_cells(&html),
        ["Culet", "P1", "P2", "G1", "C1", "C2", "C3", "T"]
    );
    assert_eq!(
        label_cells(section_html(&html, "Pavilion")),
        ["Culet", "P1", "P2", "G1"]
    );
    assert_eq!(
        label_cells(section_html(&html, "Crown")),
        ["C1", "C2", "C3", "T"]
    );
}

/// A concave design's rows are the cutting order too, labelled with `tier_codes()` (a concave
/// tier continues the P/C counts), and a concave row's section is the side its angle is on.
#[test]
fn html_rows_of_a_concave_design_follow_the_cutting_order_with_codes() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let html = cutting_sheet_html(&design, &solved, None, &[]);

    let codes = design.tier_codes();
    let order = design.cutting_order();
    let expected_codes: Vec<String> = order
        .iter()
        .map(|tier| match *tier {
            TierRef::Flat(i) => codes.flat[i].code.clone(),
            TierRef::Concave(i) => codes.concave[i].code.clone(),
        })
        .collect();
    assert_eq!(label_cells(&html), expected_codes);

    // A row is in the Crown section when its tier is on the crown side; a girdle (90 degrees)
    // and everything on the pavilion side is in the Pavilion section.
    let is_crown: Vec<bool> = order
        .iter()
        .map(|tier| match *tier {
            TierRef::Flat(i) => {
                let angle = design.tiers[i].angle_deg;
                angle.abs() < 90.0 && angle >= 0.0
            }
            TierRef::Concave(i) => design.concave_tiers[i].is_crown_side(),
        })
        .collect();
    let section_of = |wanted: bool| -> Vec<String> {
        expected_codes
            .iter()
            .zip(&is_crown)
            .filter(|&(_, &crown)| crown == wanted)
            .map(|(code, _)| code.clone())
            .collect()
    };
    assert_eq!(
        label_cells(section_html(&html, "Pavilion")),
        section_of(false)
    );
    assert_eq!(label_cells(section_html(&html, "Crown")), section_of(true));
}

/// A concave tier has no mast: its mast cells in the mast table read `-` (as the text sheet's
/// do).
#[test]
fn html_concave_rows_print_a_dash_for_the_mast() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let masts = html
        .split("<table class=\"masts\">")
        .nth(1)
        .expect("the mast table");
    let codes = design.tier_codes();
    for concave in &codes.concave {
        let row = masts
            .split("<tr>")
            .find(|row| row.starts_with(&format!("<td>{}</td>", concave.code)))
            .unwrap_or_else(|| panic!("{} in the mast table", concave.code));
        assert!(
            row.contains("<td>-</td><td>-</td>"),
            "mast and mast (mm) of a concave tier: {row}"
        );
    }
}

/// The index column is the template's: dash-separated, two digits per position, index 0
/// written as the gear's tooth count, `-` for a tier with no list.
#[test]
fn html_index_cells_are_dash_separated_and_print_index_zero_as_the_gear() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    assert_eq!(design.meta.gear_teeth_abs(), 96);
    assert!(
        html.contains("<td class=\"indices\">96-12-24-36-48-60-72-84</td>"),
        "{html}"
    );
    assert!(!html.contains(">00-12-24"), "index 0 is the tooth count");
    assert!(
        html.contains("<td class=\"indices\">06-18-30-42-54-66-78-90</td>"),
        "{html}"
    );
    assert!(
        html.contains("<td class=\"indices\">95-01-11-13-23-25-35-37-47-49-59-61-71-73-83-85</td>"),
        "{html}"
    );
    assert!(
        html.contains("<td class=\"indices\">-</td>"),
        "an empty list prints a dash"
    );
}

/// A Meet instruction names its targets by code; the template's instruction column carries no
/// tier name in front of it.
#[test]
fn html_meet_cells_use_codes_without_the_tier_name() {
    let mut design = round_brilliant_design();
    // The sheet only needs one solved entry per tier, so the pinned fixture's own masts stand
    // in while the girdle (stored 4) is changed to meet the two main tiers by their names.
    let solved = design.solve().expect("every tier is pinned");
    design.tiers[4].constraint =
        pleochroma::geometry::meet_solver::MeetConstraint::MeetNamed(vec![
            "Pavilion Main".to_string(),
            "Crown Main".to_string(),
        ]);
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::from_design(&design));
    let girdle = layout
        .sheet
        .rows
        .iter()
        .find(|row| row.code == "G1")
        .expect("the girdle row");
    let instruction = template_instruction(girdle);
    assert!(!instruction.starts_with("Girdle:"), "{instruction}");
    assert!(html.contains(&instruction_cell(&instruction)), "{html}");
    // The cut-core instruction keeps the name in front; the template strips exactly that.
    assert_eq!(girdle.instruction(), format!("Girdle: {instruction}"));
}

/// The millimetre figures of a concave tool in the mast table are the resolver's
/// width-relative ratios times the stone width, which `mm_per_unit` anchors to the girdle
/// diameter. On an elongated stone (width axis well below the length axis) they still equal
/// `ratio * mm_per_unit * width_axis`, i.e. `ratio * girdle_diameter_mm`.
#[test]
fn html_tool_row_millimetres_use_the_stone_width_on_an_elongated_stone() {
    let mut design = Design::concave_fixture();
    design.preform = PreformSpec::block(0.7, 1.8, 2.0);
    design.girdle_diameter_mm = Some(7.0);
    let solved = design.solve().expect("the elongated fixture solves");
    let metrics = design
        .measure_from_solved_geom(&solved)
        .expect("resolves")
        .expect("closed stone");
    assert!(
        metrics.length_axis > 1.2 * metrics.width_axis,
        "the stone must be elongated: {} x {}",
        metrics.width_axis,
        metrics.length_axis
    );
    let width_mm = design.yield_report(&solved).mm_per_unit.expect("scale") * metrics.width_axis;
    let html = cutting_sheet_html(&design, &solved, None, &[]);
    for tier in &design.concave_tiers {
        let expected = format!("(D = {:.2} mm)", tier.diameter_ratio * width_mm);
        assert!(html.contains(&expected), "{expected} missing");
    }
}
