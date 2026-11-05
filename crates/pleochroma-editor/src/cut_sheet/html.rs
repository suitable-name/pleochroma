//! Builds a design's printable cutting instructions as one self-contained HTML string.
//!
//! The main part follows the official cutting-instructions template block for block, in the
//! template's order, columns and rule weights:
//!
//! - a left column (37 % of the width): the title, a thick rule, the subtitle, a thin rule,
//!   Author | Month and year, then Facet Data, Size Data and Design Data, each a bold heading
//!   over a thick rule and two halves split by a vertical rule;
//! - to its right (58 %), as tall as the left column, a 2 x 2 grid of the four views as line
//!   drawings (inline SVG from `pleochroma_solid::diagram2d::view_to_svg`): `down +Z` top left,
//!   `down +X` top right, `down -Y` bottom left, `down -Z` bottom right;
//! - Comments: a bold heading over a thick rule, the comment lines centred across the page;
//! - Pavilion and Crown: a large heading with a short thick underline, then one line per tier
//!   in cutting order on alternating grey bands (code, angle, index list, instruction).
//!
//! The print stylesheet sets US Letter with 0.4 in margins, so printing the page gives the
//! template. After the main part, an "Additional information" section (HTML only, on a new
//! printed page) holds, under headings in the template's style (bold over a thick rule):
//!
//! - Renderings: the shaded renderings of the four views at print resolution, 2 x 2, each
//!   captioned with its name and direction (`Crown, view down +Z`);
//! - Stone summary: the size in millimetres, the proportions, the material, the refractive index
//!   and the carat estimate with its cube-of-width scaling ([`super::StoneSummary`]), label and
//!   figure split by a vertical rule as in Design Data;
//! - Orientation, when the stone's crystal axis is set;
//! - Masts: every tier's mast on the template's alternating grey bands, numbers right-aligned.
//!
//! On screen each printed page is a white Letter sheet (the same 7.7 in column inside 0.4 in of
//! padding) on a grey ground, the additional information on a sheet of its own, under a small
//! screen-only bar with the heading. `@media print` takes the sheets and the bar away again. The
//! design's notes are not repeated there: every one is already in the template's Comments.
//!
//! Every number and row comes from [`SheetLayout`]; this module only writes it as HTML.

use super::{
    VECTOR_VIEW_HEIGHT, VECTOR_VIEW_WIDTH,
    details::SheetDetails,
    diagram::{CutViews, DiagramImage, render_vector_cut_views},
    layout::{SectionLine, SheetLayout},
    orientation::push_orientation,
};
use pleochroma::{
    geometry::meet_solver::{Block, SolvedTier},
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{CrystalOrientation, CutSheetRow, Design};
use pleochroma_solid::diagram2d::{VectorView, view_to_svg};
use std::fmt::Write as _;

/// The standard (RFC 4648) base64 alphabet, padded -- this crate's own encoder
/// rather than a new dependency (`base64` is not a workspace dependency
/// anywhere in this repo). A `data:` URI is the only place this codebase needs base64 at all,
/// so a ~20-line hand-rolled encoder is simpler than justifying a new crate for it.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `data` as base64 with `=` padding, per RFC 4648 -- see
/// [`BASE64_ALPHABET`]'s own doc comment for why this is hand-rolled.
///
/// `pub(super)` so this module's own tests (`cut_sheet::tests`) can check the
/// encoder directly against known vectors.
#[must_use]
pub(super) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(BASE64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Builds `image`'s `data:` URI -- what keeps the sheet a single self-contained file, rather
/// than an HTML file plus sibling PNGs that a cutter could separate or lose.
fn png_data_uri(image: &DiagramImage) -> String {
    format!("data:image/png;base64,{}", base64_encode(&image.png_bytes))
}

/// Escapes `s` for safe interpolation into HTML text/attribute content -- a
/// tier name or meet note is free text a cutter authored (or a `.asc` file's own
/// `G`-field text, verbatim -- see `CutSheetRow::meet_instruction`'s doc
/// comment), never assumed free of `&`/`<`/`>`/quote characters.
///
/// `pub(super)` so this module's own tests can check the escaping directly.
pub(super) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The labels of the template's four views, in grid order (top left, top right, bottom left,
/// bottom right), which is also the order of `render_vector_cut_views` and of [`CutViews`]'
/// fields `top`, `side`, `end`, `bottom`.
pub(super) const VIEW_LABELS: [&str; 4] = ["down +Z", "down +X", "down -Y", "down -Z"];

/// The names of the four views in [`VIEW_LABELS`]' order, as the line drawings caption them.
const VIEW_NAMES: [&str; 4] = ["Crown", "Profile", "End", "Pavilion"];

/// The caption of the legacy single picture ([`cutting_sheet_html`] with a diagram).
const DIAGRAM_CAPTION: &str = "2D faceting diagram";

/// What the screen-only bar says the page is, after the heading.
pub(super) const SCREEN_BAR_TEXT: &str = "Cutting instructions \u{b7} Pleochroma";

/// The shaded renderings the additional information shows.
#[derive(Clone, Copy)]
enum Renderings<'a> {
    /// None: the design does not close, or the caller has none.
    Missing,
    /// One picture, the legacy three-panel diagram ([`cutting_sheet_html`]).
    Single(&'a DiagramImage),
    /// The four shaded views.
    Four(&'a CutViews),
}

/// Builds `design`'s printable cutting instructions as a single self-contained HTML string,
/// with the header words from [`SheetDetails::from_design`].
///
/// The template's four line drawings are drawn from `design` itself. `diagram` is one
/// already-rendered shaded picture (build one via [`super::diagram::render_cut_diagram`]),
/// printed in the additional information; `None` prints none. The four shaded views come from
/// [`cutting_sheet_html_with`].
///
/// Pure: same `design`/`solved`/`diagram`/`custom` in always produces the same
/// string out, no file/dialog/clock access -- see this module's own doc comment.
///
/// `custom` -- a caller's own resolved catalogue materials, e.g.
/// `RenderContext::custom_materials` -- is consulted for the printed "RI range" line exactly
/// like [`Design::cutting_sheet_with`]/[`Design::effective_refractive_index_with`], so a design
/// on a CUSTOM catalogue material (not one of the built-ins) prints that material's own `n_D`
/// rather than the legacy schedule RI. Pass `&[]` for a caller with no catalogue on hand.
///
/// # Panics
///
/// Same alignment contract as [`Design::cutting_sheet`]: `solved` must have one
/// entry per tier `design` currently has, in the same order.
#[must_use]
pub fn cutting_sheet_html(
    design: &Design,
    solved: &[SolvedTier],
    diagram: Option<&DiagramImage>,
    custom: &[GemMaterial],
) -> String {
    let layout = SheetLayout::build(design, solved, custom, &SheetDetails::from_design(design));
    let vectors = render_vector_cut_views(design, solved, VECTOR_VIEW_WIDTH, VECTOR_VIEW_HEIGHT);
    build_html(
        &layout,
        vectors.as_ref(),
        diagram.map_or(Renderings::Missing, Renderings::Single),
    )
}

/// Builds `design`'s printable cutting instructions with explicit header `details`; `views`
/// are the four shaded renderings of the additional information (`None` prints none).
///
/// The main part is the template (see this module's doc comment); the views of its grid are
/// line drawings drawn from `design` itself, and a design that does not close prints a notice
/// in their place.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_html`].
#[must_use]
pub fn cutting_sheet_html_with(
    design: &Design,
    solved: &[SolvedTier],
    views: Option<&CutViews>,
    custom: &[GemMaterial],
    details: &SheetDetails,
) -> String {
    cutting_sheet_html_with_default(design, solved, views, custom, None, details)
}

/// [`cutting_sheet_html_with`] with the stored default orientation of the design's material.
///
/// The orientation is `LibraryMaterial::default_orientation` of its library row, as for
/// [`SheetLayout::build_with_default`]. With `None` it is [`cutting_sheet_html_with`].
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_html`].
#[must_use]
pub fn cutting_sheet_html_with_default(
    design: &Design,
    solved: &[SolvedTier],
    views: Option<&CutViews>,
    custom: &[GemMaterial],
    stored_default: Option<CrystalOrientation>,
    details: &SheetDetails,
) -> String {
    let layout = SheetLayout::build_with_default(design, solved, custom, stored_default, details);
    let vectors = render_vector_cut_views(design, solved, VECTOR_VIEW_WIDTH, VECTOR_VIEW_HEIGHT);
    build_html(
        &layout,
        vectors.as_ref(),
        views.map_or(Renderings::Missing, Renderings::Four),
    )
}

/// The whole page for `layout`: the screen-only bar, the template, then the additional
/// information.
fn build_html(
    layout: &SheetLayout,
    vectors: Option<&[VectorView; 4]>,
    renderings: Renderings<'_>,
) -> String {
    let mut out = HTML_HEAD.replacen("{TITLE}", &html_escape(layout.heading()), 1);
    push_screen_bar(&mut out, layout);
    out.push_str("<main class=\"template\">\n<div class=\"top\">\n<div class=\"left\">\n");
    push_header(&mut out, layout);
    push_facet_data(&mut out, layout);
    push_size_data(&mut out, layout);
    push_design_data(&mut out, layout);
    out.push_str("</div>\n");
    push_views(&mut out, vectors);
    out.push_str("</div>\n");
    push_comments(&mut out, layout);
    push_section(&mut out, "Pavilion", &layout.pavilion_lines());
    push_section(&mut out, "Crown", &layout.crown_lines());
    out.push_str("</main>\n");
    push_additional(&mut out, layout, renderings);
    out.push_str(HTML_TAIL);
    out
}

/// The bar above the first sheet: the heading, then what the page is. Screen only: the print
/// rules of [`HTML_HEAD`] hide it, and it sits outside the template's `<main>`, so the printed
/// template is exactly the standard.
fn push_screen_bar(out: &mut String, layout: &SheetLayout) {
    let _ = writeln!(
        out,
        "<header class=\"screen-bar\"><span class=\"bar-title\">{}</span>\
         <span class=\"bar-what\">{SCREEN_BAR_TEXT}</span></header>",
        html_escape(layout.heading())
    );
}

/// The title, a thick rule, the subtitle, a thin rule, then Author | Month and year.
///
/// The template's lines are always there; an empty field leaves its line blank.
fn push_header(out: &mut String, layout: &SheetLayout) {
    let details = &layout.details;
    let _ = writeln!(
        out,
        "<h1 class=\"title\">{}</h1>\n<div class=\"rule-thick\"></div>\n\
         <p class=\"subtitle\">{}</p>\n<div class=\"rule-thin\"></div>\n\
         <div class=\"pair\"><p class=\"author\">{}</p><p class=\"date\">{}</p></div>",
        html_escape(layout.heading()),
        blank_or(&html_escape(details.subtitle.trim())),
        blank_or(&html_escape(details.author.trim())),
        blank_or(&html_escape(details.date_text.trim())),
    );
}

/// `text`, or a non-breaking space for an empty one, so the template's line keeps its height.
const fn blank_or(text: &str) -> &str {
    if text.is_empty() { "&nbsp;" } else { text }
}

/// One block of the left column: its heading over a thick rule, then rows of four cells split
/// in the middle by a vertical rule (`class` names the block's column widths).
fn push_four_column_block(out: &mut String, heading: &str, class: &str, rows: &[[String; 4]]) {
    let _ = writeln!(
        out,
        "<h2 class=\"block\">{heading}</h2>\n<table class=\"block {class}\">"
    );
    for [label, value, right_label, right_value] in rows {
        let _ = writeln!(
            out,
            "<tr><td>{label}</td><td class=\"split\">{value}</td>\
             <td>{right_label}</td><td>{right_value}</td></tr>"
        );
    }
    out.push_str("</table>\n");
}

/// Facet Data: facets on the left, tiers on the right; the crown as `N+1` with a table.
fn push_facet_data(out: &mut String, layout: &SheetLayout) {
    let rows: Vec<[String; 4]> = layout
        .facets
        .template_rows()
        .into_iter()
        .map(|(label, facets, tier_label, tiers)| {
            [label.to_string(), facets, tier_label.to_string(), tiers]
        })
        .collect();
    push_four_column_block(out, "Facet Data", "facets", &rows);
}

/// Size Data: `L/W`, `H/W`, `V/W³` on the left, `P/W`, `C/W`, `P/C` on the right, three
/// decimals.
fn push_size_data(out: &mut String, layout: &SheetLayout) {
    let cells = layout.sizes.cells();
    let rows: Vec<[String; 4]> = (0..3)
        .map(|i| {
            [
                html_escape(cells[i].0),
                cells[i].1.clone(),
                html_escape(cells[i + 3].0),
                cells[i + 3].1.clone(),
            ]
        })
        .collect();
    push_four_column_block(out, "Size Data", "sizes", &rows);
}

/// Design Data: the template's five rows, label | value.
fn push_design_data(out: &mut String, layout: &SheetLayout) {
    out.push_str("<h2 class=\"block\">Design Data</h2>\n<table class=\"block design\">\n");
    for (label, value) in &layout.design_data {
        let _ = writeln!(
            out,
            "<tr><td class=\"split\">{}</td><td>{}</td></tr>",
            html_escape(label),
            html_escape(value)
        );
    }
    out.push_str("</table>\n");
}

/// The 2 x 2 grid of the four line drawings, or the "not a closed solid" notice.
fn push_views(out: &mut String, vectors: Option<&[VectorView; 4]>) {
    out.push_str("<div class=\"views\">\n");
    match vectors {
        Some(views) => {
            for (label, view) in VIEW_LABELS.iter().zip(views) {
                let _ = writeln!(
                    out,
                    "<div class=\"cell\" title=\"view {label}\">{}</div>",
                    view_to_svg(view)
                );
            }
        }
        None => out.push_str(
            "<p class=\"diagram-missing\">Diagram unavailable: this design does not \
             currently close to a solid.</p>\n",
        ),
    }
    out.push_str("</div>\n");
}

/// The views grid alone, for the tests: a closed design always has its drawings, so the notice
/// is only reachable with `None`.
#[cfg(test)]
pub(super) fn views_block(vectors: Option<&[VectorView; 4]>) -> String {
    let mut out = String::new();
    push_views(&mut out, vectors);
    out
}

/// Comments: the heading over a thick rule, then each comment line centred across the page;
/// nothing at all when there are none.
fn push_comments(out: &mut String, layout: &SheetLayout) {
    if layout.details.comments.is_empty() {
        return;
    }
    out.push_str("<section class=\"comments\">\n<h2 class=\"block\">Comments</h2>\n");
    for comment in &layout.details.comments {
        let _ = writeln!(out, "<p>{}</p>", html_escape(comment));
    }
    out.push_str("</section>\n");
}

/// One section (`Pavilion` or `Crown`): its heading with the short underline, then its lines on
/// alternating bands. Nothing for a section with no lines.
fn push_section(out: &mut String, heading: &str, lines: &[SectionLine]) {
    if lines.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "<section class=\"cut-section\">\n<h2 class=\"section\">{heading}</h2>\n\
         <table class=\"rows\">\n<colgroup><col class=\"code\"><col class=\"angle\">\
         <col class=\"indices\"><col class=\"instruction\"></colgroup>"
    );
    // One `<tbody>` per tier, so a concave tier's facet line and tool line never split.
    let mut open = false;
    for line in lines {
        if !line.tool {
            if open {
                out.push_str("</tbody>\n");
            }
            out.push_str("<tbody>\n");
            open = true;
        }
        let mut classes = Vec::new();
        if line.shaded {
            classes.push("shaded");
        }
        if line.tool {
            classes.push("tool");
        }
        let class = if classes.is_empty() {
            String::new()
        } else {
            format!(" class=\"{}\"", classes.join(" "))
        };
        let _ = writeln!(
            out,
            "<tr{class}><td class=\"code\">{}</td><td class=\"angle\">{}</td>\
             <td class=\"indices\">{}</td><td class=\"instruction\">{}</td></tr>",
            html_escape(&line.code),
            html_escape(&line.angle),
            html_escape(&line.indices),
            html_escape(&line.instruction),
        );
    }
    if open {
        out.push_str("</tbody>\n");
    }
    out.push_str("</table>\n</section>\n");
}

/// The additional information after the template, on a sheet of its own: the shaded
/// renderings, the Stone summary, the Orientation block and the mast table.
fn push_additional(out: &mut String, layout: &SheetLayout, renderings: Renderings<'_>) {
    out.push_str("<section class=\"extra\">\n<h2 class=\"extra\">Additional information</h2>\n");
    push_renderings(out, renderings);
    push_summary(out, layout);
    // How the rough must sit, for a stone whose crystal axis is set; nothing otherwise.
    if let Some(block) = &layout.sheet.orientation {
        push_orientation(out, block);
    }
    push_mast_table(out, layout);
    out.push_str("</section>\n");
}

/// The shaded renderings in a 2 x 2 grid, each scaled to its cell and captioned with the view's
/// name and direction (`Crown, view down +Z`); the legacy single picture spans the grid. Nothing
/// when there are none. The pictures link nowhere: the file stays self-contained.
fn push_renderings(out: &mut String, renderings: Renderings<'_>) {
    if matches!(renderings, Renderings::Missing) {
        return;
    }
    out.push_str("<h3>Renderings</h3>\n<div class=\"renders\">\n");
    match renderings {
        Renderings::Missing => {}
        Renderings::Single(image) => push_figure(
            out,
            "<figure class=\"wide\">",
            DIAGRAM_CAPTION,
            DIAGRAM_CAPTION,
            image,
        ),
        Renderings::Four(views) => {
            for ((name, label), image) in VIEW_NAMES.iter().zip(VIEW_LABELS).zip([
                &views.top,
                &views.side,
                &views.end,
                &views.bottom,
            ]) {
                push_figure(
                    out,
                    "<figure>",
                    &format!("{name}, view {label}"),
                    &format!("<strong>{name}</strong>, view {label}"),
                    image,
                );
            }
        }
    }
    out.push_str("</div>\n");
}

/// One rendering: `opening` (the `<figure>` tag), the picture with `alt` (plain text), then
/// `caption` (HTML) under it.
fn push_figure(out: &mut String, opening: &str, alt: &str, caption: &str, image: &DiagramImage) {
    let _ = writeln!(
        out,
        "{opening}<img src=\"{}\" width=\"{}\" height=\"{}\" alt=\"{alt}\">\
         <figcaption>{caption}</figcaption></figure>",
        png_data_uri(image),
        image.width,
        image.height
    );
}

/// The Stone summary: one line per figure of [`super::StoneSummary::rows`], the label left of a
/// vertical rule as in the template's Design Data, a caveat in smaller type under its figure.
fn push_summary(out: &mut String, layout: &SheetLayout) {
    out.push_str("<h3>Stone summary</h3>\n<table class=\"summary\">\n");
    for row in layout.summary.rows() {
        let note = row.note.map_or_else(String::new, |note| {
            format!(" <span class=\"note\">{}</span>", html_escape(note))
        });
        let _ = writeln!(
            out,
            "<tr><th>{}</th><td>{}{note}</td></tr>",
            html_escape(row.label),
            html_escape(&row.value)
        );
    }
    out.push_str("</table>\n");
}

/// The mast table: every tier in cutting order with its code, its name, its mast (model units),
/// the mast in millimetres when a girdle diameter is set, the cheater offset when some tier has
/// one, and for a concave tier its tool in millimetres when a girdle diameter is set.
///
/// The stylesheet gives it the template's look: a rule under the header, every other row on a
/// grey band from the first, the code centred and the figures right-aligned.
fn push_mast_table(out: &mut String, layout: &SheetLayout) {
    let show_mm = layout.girdle_diameter_mm.is_some();
    let show_cheater = layout.has_cheater_column();
    let show_tool = show_mm && layout.sheet.rows.iter().any(|row| row.concave.is_some());
    out.push_str("<h3>Masts</h3>\n<table class=\"masts\">\n<thead><tr>");
    out.push_str("<th>Code</th><th>Tier</th><th>Mast</th>");
    if show_mm {
        out.push_str("<th>Mast (mm)</th>");
    }
    if show_cheater {
        out.push_str("<th>Cheater</th>");
    }
    if show_tool {
        out.push_str("<th>Tool (mm)</th>");
    }
    out.push_str("</tr></thead>\n<tbody>\n");
    let rows: Vec<(&CutSheetRow, Block)> = layout
        .pavilion_rows()
        .into_iter()
        .chain(layout.crown_rows())
        .collect();
    for (row, _) in rows {
        let has_mast = row.concave.is_none();
        let mast = if has_mast {
            format!("{:.4}", row.mast)
        } else {
            "-".to_string()
        };
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{}</td><td>{mast}</td>",
            html_escape(row.label()),
            html_escape(row.descriptive_name().unwrap_or_default()),
        );
        if show_mm {
            // A concave tier has no mast; a scale that does not resolve prints a dash too.
            let cell = layout.mm_per_unit.filter(|_| has_mast).map_or_else(
                || "-".to_string(),
                |scale| format!("{:.4}", row.mast * scale),
            );
            let _ = write!(out, "<td>{cell}</td>");
        }
        if show_cheater {
            // A tier with no recorded offset reads "-", not "+0.00", so "deliberately
            // on-tooth" is distinguishable from "nobody set one".
            let cell = row
                .cheater_offset_deg
                .map_or_else(|| "-".to_string(), |deg| format!("{deg:+.2}&deg;"));
            let _ = write!(out, "<td>{cell}</td>");
        }
        if show_tool {
            let _ = write!(out, "<td>{}</td>", tool_mm_text(row, layout));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
}

/// A concave tier's tool in millimetres: the displacement and the diameter, each the
/// resolver's width-relative ratio times the stone width (the girdle diameter); `-` for a flat
/// tier or without a girdle diameter.
fn tool_mm_text(row: &CutSheetRow, layout: &SheetLayout) -> String {
    let (Some(concave), Some(width_mm)) = (&row.concave, layout.girdle_diameter_mm) else {
        return "-".to_string();
    };
    let [x, y, z] = concave.displacement.map(|ratio| ratio * width_mm);
    format!(
        "({x:.2}, {y:.2}, {z:.2} mm) (D = {:.2} mm)",
        concave.diameter_ratio * width_mm
    )
}

/// The opening half of the page: `<!DOCTYPE html>` through the stylesheet and `<body>`, with
/// `{TITLE}` standing for the escaped heading.
///
/// The template's measures are in points on a US Letter page with 0.4 in margins (a 7.7 in
/// wide column). On screen the template and the additional information are each a white sheet
/// of that page (8.5 in wide, at least 11 in tall, 0.4 in of padding) on a light grey ground,
/// with a soft shadow and a gap between them, under the screen-only bar. `@media print` takes
/// the sheets, the grey and the bar away, so the printout is the plain Letter pages with the
/// `@page` margins. The grey bands print (`print-color-adjust: exact`), a tier's lines never
/// split across a page break, and the additional information starts on a page of its own.
const HTML_HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{TITLE}</title>
<style>
  :root { color-scheme: light; }
  * { box-sizing: border-box; }
  body {
    font-family: Helvetica, Arial, sans-serif;
    background: #e5e7eb;
    color: #000000;
    margin: 0;
    padding: 0 24px 40px;
    -webkit-print-color-adjust: exact;
    print-color-adjust: exact;
  }
  header.screen-bar {
    width: 8.5in; margin: 0 auto; padding: 14px 2px 10px;
    display: flex; justify-content: space-between; align-items: baseline; gap: 16px;
    font-size: 12px; color: #5f6672;
  }
  header.screen-bar .bar-title { font-weight: 700; color: #1f2329; }
  main.template, section.extra {
    width: 8.5in; min-height: 11in; margin: 0 auto 28px; padding: 0.4in;
    background: #ffffff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12), 0 8px 24px rgba(0, 0, 0, 0.10);
  }
  p { margin: 0; }
  .top { display: flex; justify-content: space-between; align-items: stretch; }
  .left { width: 37%; }
  h1.title {
    font-size: 18pt; font-weight: 700; text-align: center;
    margin: 0; min-height: 41pt; padding: 12pt 0 6pt;
  }
  .rule-thick { border-top: 2.5pt solid #000000; }
  .rule-thin { border-top: 1pt solid #000000; }
  p.subtitle { font-size: 11pt; text-align: center; padding: 7pt 0 6pt; }
  .pair { display: flex; font-size: 11pt; }
  .pair p { width: 50%; text-align: center; padding: 1pt 0 2pt; }
  .pair p.author { border-right: 1pt solid #000000; }
  h2.block {
    font-size: 12pt; font-weight: 700; text-align: center;
    margin: 16pt 0 0; padding-bottom: 1pt; border-bottom: 2.5pt solid #000000;
  }
  table.block { width: 100%; border-collapse: collapse; table-layout: fixed; }
  table.block td { text-align: center; height: 13.9pt; padding: 0 2pt; white-space: nowrap; }
  table.block td.split { border-right: 1pt solid #000000; }
  table.facets td { font-size: 8pt; }
  table.facets td:nth-child(1) { width: 34%; }
  table.facets td:nth-child(2) { width: 16%; }
  table.facets td:nth-child(3) { width: 38%; }
  table.facets td:nth-child(4) { width: 12%; }
  table.sizes td, table.design td { font-size: 10pt; }
  table.sizes td:nth-child(1), table.sizes td:nth-child(4) { width: 17%; }
  table.sizes td:nth-child(2), table.sizes td:nth-child(3) { width: 33%; }
  table.design td { width: 50%; white-space: normal; }
  .views {
    width: 58%; display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
    border: 1.2pt solid #000000;
  }
  .views .cell { position: relative; min-height: 0; }
  .views .cell:nth-child(odd) { border-right: 1.2pt solid #000000; }
  .views .cell:nth-child(-n+2) { border-bottom: 1.2pt solid #000000; }
  .views .cell svg {
    position: absolute; left: 4pt; top: 4pt;
    width: calc(100% - 8pt); height: calc(100% - 8pt);
  }
  .views .diagram-missing {
    grid-column: 1 / span 2; grid-row: 1 / span 2; align-self: center;
    text-align: center; color: #b91c1c; font-style: italic; padding: 12pt;
  }
  section.comments h2.block { width: 37%; }
  section.comments p { font-size: 9pt; text-align: center; margin-top: 3pt; }
  h2.section {
    font-size: 24pt; font-weight: 700; margin: 24pt 0 0; padding: 0 0 2pt 9pt;
    display: inline-block; min-width: 77pt; border-bottom: 2.5pt solid #000000;
    break-after: avoid; page-break-after: avoid;
  }
  table.rows { width: 100%; border-collapse: collapse; table-layout: fixed; font-size: 10pt; }
  table.rows col.code { width: 5.6%; }
  table.rows col.angle { width: 6.5%; }
  table.rows col.indices { width: 39.6%; }
  table.rows col.instruction { width: 48.3%; }
  table.rows td { height: 13.9pt; padding: 1pt 0; vertical-align: middle; }
  table.rows td.code { text-align: center; }
  table.rows td.angle { text-align: right; white-space: nowrap; }
  table.rows td.indices { padding-left: 9pt; padding-right: 6pt; overflow-wrap: anywhere; }
  table.rows tr.shaded td { background: #d9d9d9; }
  table.rows tbody { break-inside: avoid; page-break-inside: avoid; }
  section.extra { break-before: page; page-break-before: always; }
  h2.extra {
    font-size: 24pt; font-weight: 700; margin: 0; padding: 0 0 2pt 9pt;
    display: inline-block; min-width: 77pt; border-bottom: 2.5pt solid #000000;
  }
  section.extra h3, section.orientation h2 {
    width: 37%; font-size: 12pt; font-weight: 700; text-align: center;
    margin: 20pt 0 6pt; padding-bottom: 1pt; border-bottom: 2.5pt solid #000000;
    break-after: avoid; page-break-after: avoid;
  }
  .renders { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 10pt 12pt; }
  .renders figure { margin: 0; break-inside: avoid; page-break-inside: avoid; }
  .renders figure.wide { grid-column: 1 / -1; }
  .renders img { display: block; width: 100%; height: auto; }
  .renders figcaption { font-size: 9pt; text-align: center; margin-top: 3pt; }
  table.summary { width: 100%; border-collapse: collapse; table-layout: fixed; font-size: 10pt; }
  table.summary th, table.summary td { height: 13.9pt; vertical-align: top; }
  table.summary th {
    width: 37%; font-weight: 400; text-align: center;
    padding: 1pt 6pt; border-right: 1pt solid #000000;
  }
  table.summary td { padding: 1pt 0 1pt 9pt; }
  table.summary .note { display: block; font-size: 8pt; color: #4b5058; padding: 1pt 0 2pt; }
  section.orientation { break-inside: avoid; page-break-inside: avoid; }
  section.orientation .orientation-body { display: flex; gap: 16pt; align-items: center; }
  section.orientation ul { flex: 1; margin: 0; padding-left: 14pt; font-size: 10pt; line-height: 1.35; }
  section.orientation li + li { margin-top: 3pt; }
  section.orientation svg { flex: none; width: 3.8in; height: auto; }
  table.masts { width: 100%; border-collapse: collapse; font-size: 10pt; }
  table.masts th {
    font-weight: 700; text-align: left; white-space: nowrap;
    padding: 0 6pt 2pt; border-bottom: 1pt solid #000000;
  }
  table.masts td { height: 13.9pt; padding: 1pt 6pt; vertical-align: middle; }
  table.masts th:first-child, table.masts td:first-child { width: 5.6%; text-align: center; }
  table.masts th:nth-child(n+3), table.masts td:nth-child(n+3) {
    text-align: right; width: 13%; white-space: nowrap; font-variant-numeric: tabular-nums;
  }
  table.masts tbody tr:nth-child(odd) td { background: #d9d9d9; }
  table.masts tbody tr { break-inside: avoid; page-break-inside: avoid; }
  @page { size: letter; margin: 0.4in; }
  @media print {
    body { margin: 0; padding: 0; background: #ffffff; }
    header.screen-bar { display: none; }
    main.template, section.extra {
      width: auto; min-height: 0; margin: 0; padding: 0; background: #ffffff; box-shadow: none;
    }
  }
</style>
</head>
<body>
"#;

/// The closing half of the page.
const HTML_TAIL: &str = "</body>\n</html>\n";
