//! The cutting instructions as a PDF: the official template, drawn with `pdf-writer` on US
//! Letter pages.
//!
//! The page is the HTML sheet's main part ([`super::html`]) block for block, from the same
//! [`SheetLayout`]:
//!
//! - the left column (37 % of the width): title, thick rule, subtitle, thin rule, Author |
//!   Month and year, Facet Data, Size Data and Design Data;
//! - the 2 x 2 grid of the four views beside it (58 %), as tall as the left column, each view
//!   the vector paths of its [`VectorView`] fitted into its cell with its aspect kept;
//! - Comments, centred across the page;
//! - the Pavilion and the Crown section: a 24 pt heading with a short thick underline, then the
//!   tier lines on alternating grey (`#d9d9d9`) bands, the shading a filled rectangle.
//!
//! Text is Helvetica and Helvetica-Bold, two of the standard 14 fonts, in `WinAnsiEncoding`
//! with nothing embedded ([`super::pdf_font`] measures it). When the Pavilion or Crown lines do
//! not fit on the first page they continue on further pages, each starting with the section's
//! heading and "(continued)". A concave tier's facet and tool lines stay on one page.
//!
//! The output is deterministic: no date, no document ID, uncompressed streams.

use super::{
    layout::{SectionLine, SheetLayout},
    pdf_font::{Font, encode, text_width},
};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str, types::LineCapStyle};
use pleochroma_solid::diagram2d::{BASELINE_DROP_EMS, LineWeight, VectorView};

/// US Letter, portrait, in points.
const PAGE_WIDTH: f32 = 612.0;
/// See [`PAGE_WIDTH`].
const PAGE_HEIGHT: f32 = 792.0;
/// The page margin on every side: 0.4 in.
const MARGIN: f32 = 28.8;
/// The width between the margins.
const CONTENT_WIDTH: f32 = PAGE_WIDTH - 2.0 * MARGIN;
/// The lowest a line may reach before the next page starts.
const BOTTOM: f32 = PAGE_HEIGHT - MARGIN;
/// The left column's width: 37 % of the content width.
const LEFT_WIDTH: f32 = 0.37 * CONTENT_WIDTH;
/// The views grid's left edge: the left column, then a 5 % gap.
const VIEWS_X: f32 = MARGIN + 0.42 * CONTENT_WIDTH;
/// The views grid's width: 58 % of the content width.
const VIEWS_WIDTH: f32 = 0.58 * CONTENT_WIDTH;
/// A thick rule (under the title and every block heading, the section underline).
const THICK: f32 = 2.5;
/// A thin rule (under the subtitle, the vertical rules of the data blocks).
const THIN: f32 = 1.0;
/// The views grid's lines.
const GRID: f32 = 1.2;
/// The height of one row of a data block or a section.
const ROW_PITCH: f32 = 13.9;
/// The extra height of every further text line of a wrapped section row.
const LINE_PITCH: f32 = 11.5;
/// Where a single-line row's baseline sits below the row's top.
const ROW_BASELINE: f32 = 10.4;
/// A shaded band's grey (`#d9d9d9`).
const SHADE: f32 = 217.0 / 255.0;
/// The font size of a section row.
const ROW_FONT: f32 = 10.0;
/// The smallest size an index list is shrunk to before it wraps.
const MIN_INDEX_FONT: f32 = 8.0;
/// A section row's code column: its centre, from the left margin.
const CODE_CENTRE: f32 = 15.8;
/// The code column's width.
const CODE_WIDTH: f32 = 31.0;
/// The angle column's right edge, from the left margin.
const ANGLE_RIGHT: f32 = 68.5;
/// The index column's left edge, from the left margin.
const INDEX_LEFT: f32 = 78.4;
/// The instruction column's left edge, from the left margin: 51.7 % of the content width.
const INSTRUCTION_LEFT: f32 = 0.517 * CONTENT_WIDTH;
/// The index column's width, a small gap before the instruction column.
const INDEX_WIDTH: f32 = INSTRUCTION_LEFT - INDEX_LEFT - 6.0;
/// The instruction column's width.
const INSTRUCTION_WIDTH: f32 = CONTENT_WIDTH - INSTRUCTION_LEFT - 4.0;

/// The PDF of `layout` with the four line drawings `views` (in the template's grid order).
///
/// The bytes are a complete PDF 1.7 file: one page for a design whose sections fit, more when
/// they continue.
#[must_use]
pub(super) fn sheet_pdf(layout: &SheetLayout, views: &[VectorView; 4]) -> Vec<u8> {
    write_pdf(sheet_pages(layout, views))
}

/// The page contents of the sheet, one [`Content`] per page.
pub(super) fn sheet_pages(layout: &SheetLayout, views: &[VectorView; 4]) -> Vec<Content> {
    let mut canvas = Canvas::new();
    let left_bottom = draw_left_column(&mut canvas, layout);
    draw_views(&mut canvas, views, left_bottom);
    let mut y = draw_comments(&mut canvas, &layout.details.comments, left_bottom);
    y = draw_section(&mut canvas, "Pavilion", &layout.pavilion_lines(), y);
    draw_section(&mut canvas, "Crown", &layout.crown_lines(), y);
    canvas.pages()
}

/// The file around `pages`: catalog, page tree, the two fonts, one page and one content stream
/// per page.
fn write_pdf(pages: Vec<Content>) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let tree_id = Ref::new(2);
    let font_ids = [(Font::Regular, Ref::new(3)), (Font::Bold, Ref::new(4))];
    let ids: Vec<(Ref, Ref)> = (0..pages.len())
        .map(|index| {
            let first = i32::try_from(5 + 2 * index).expect("a sheet has far fewer pages");
            (Ref::new(first), Ref::new(first + 1))
        })
        .collect();

    pdf.catalog(catalog_id).pages(tree_id);
    pdf.pages(tree_id)
        .kids(ids.iter().map(|&(page, _)| page))
        .count(i32::try_from(ids.len()).expect("a sheet has far fewer pages"));
    for (font, id) in font_ids {
        pdf.type1_font(id)
            .base_font(Name(font.base_font()))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
    }
    for (content, &(page_id, content_id)) in pages.into_iter().zip(&ids) {
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT));
        page.parent(tree_id);
        page.contents(content_id);
        page.resources()
            .fonts()
            .pair(Name(Font::Regular.resource()), font_ids[0].1)
            .pair(Name(Font::Bold.resource()), font_ids[1].1);
        page.finish();
        let bytes = content.finish();
        pdf.stream(content_id, &bytes).finish();
    }
    pdf.finish()
}

/// The pages being drawn. Every coordinate is in points from the page's top-left corner, `y`
/// down, as the template is measured; [`Canvas::flip`] turns it into PDF space.
struct Canvas {
    /// The finished pages.
    done: Vec<Content>,
    /// The page being drawn.
    content: Content,
}

impl Canvas {
    fn new() -> Self {
        Self {
            done: Vec::new(),
            content: Content::new(),
        }
    }

    /// Starts a new page.
    fn new_page(&mut self) {
        let page = std::mem::replace(&mut self.content, Content::new());
        self.done.push(page);
    }

    /// Every page, the one being drawn last.
    fn pages(self) -> Vec<Content> {
        let Self { mut done, content } = self;
        done.push(content);
        done
    }

    /// A top-down `y` as PDF's bottom-up one.
    const fn flip(y: f32) -> f32 {
        PAGE_HEIGHT - y
    }

    /// `text` in `font` at `size` points, its baseline starting at `(x, baseline)`.
    fn text(&mut self, font: Font, size: f32, x: f32, baseline: f32, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = encode(text);
        self.content.begin_text();
        self.content.set_font(Name(font.resource()), size);
        self.content.next_line(x, Self::flip(baseline));
        self.content.show(Str(&bytes));
        self.content.end_text();
    }

    /// `text` centred on `centre`.
    fn text_centred(&mut self, font: Font, size: f32, centre: f32, baseline: f32, text: &str) {
        let width = text_width(text, font, size);
        self.text(font, size, centre - width / 2.0, baseline, text);
    }

    /// `text` ending at `right`.
    fn text_right(&mut self, font: Font, size: f32, right: f32, baseline: f32, text: &str) {
        let width = text_width(text, font, size);
        self.text(font, size, right - width, baseline, text);
    }

    /// A straight line from `from` to `to`, `width` points wide.
    fn line(&mut self, from: (f32, f32), to: (f32, f32), width: f32) {
        self.content.set_line_width(width);
        self.content.move_to(from.0, Self::flip(from.1));
        self.content.line_to(to.0, Self::flip(to.1));
        self.content.stroke();
    }

    /// A horizontal rule from `x0` to `x1` centred on `y`.
    fn rule(&mut self, x0: f32, x1: f32, y: f32, width: f32) {
        self.line((x0, y), (x1, y), width);
    }

    /// A grey rectangle with its top-left corner at `(x, y)`.
    fn shade(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.content.set_fill_gray(SHADE);
        self.content.rect(x, Self::flip(y + height), width, height);
        self.content.fill_nonzero();
        self.content.set_fill_gray(0.0);
    }
}

/// `text` shrunk from `size` down to `min` (never below) so it is at most `width` wide.
fn shrink_to_fit(text: &str, font: Font, size: f32, min: f32, width: f32) -> f32 {
    let natural = text_width(text, font, size);
    if natural <= width || natural <= 0.0 {
        size
    } else {
        (size * width / natural).max(min)
    }
}

/// The pieces `text` may be broken into: each ends after a space or a dash (an index list
/// breaks after one of its dashes).
fn break_units(text: &str) -> Vec<&str> {
    let mut units = Vec::new();
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if c == ' ' || c == '-' {
            let end = at + c.len_utf8();
            units.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        units.push(&text[start..]);
    }
    units
}

/// `text` wrapped into lines at most `width` wide in `font` at `size` (a unit longer than the
/// width keeps a line of its own). Never empty: an empty text is one empty line.
fn wrap(text: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for unit in break_units(text) {
        let candidate = format!("{line}{unit}");
        if !line.trim().is_empty() && text_width(candidate.trim_end(), font, size) > width {
            lines.push(line.trim_end().to_string());
            line = unit.trim_start().to_string();
        } else {
            line = candidate;
        }
    }
    if !line.trim().is_empty() || lines.is_empty() {
        lines.push(line.trim_end().to_string());
    }
    lines
}

/// A block heading of the left column (`Facet Data`): bold 12 pt, centred over `width` from
/// `x0`, with a thick rule under it. Returns the top of the block's first row.
fn block_heading(canvas: &mut Canvas, heading: &str, x0: f32, width: f32, y: f32) -> f32 {
    let baseline = y + 22.0;
    canvas.text_centred(Font::Bold, 12.0, x0 + width / 2.0, baseline, heading);
    let rule = baseline + 5.0;
    canvas.rule(x0, x0 + width, rule, THICK);
    rule + THICK / 2.0
}

/// The widest a left-column cell's text may be when it is centred on `centre` (a fraction of
/// the column's width): twice the distance to the nearer edge of its half, less a margin.
fn cell_room(centre: f32) -> f32 {
    let half = if centre <= 0.5 {
        centre.min(0.5 - centre)
    } else {
        (centre - 0.5).min(1.0 - centre)
    };
    (2.0 * half).mul_add(LEFT_WIDTH, -4.0)
}

/// Rows of a left-column block from `top`, each cell's text centred on its own fraction of the
/// column's width (`centres`) and shrunk to its room, with a vertical rule down the middle.
fn block_rows(canvas: &mut Canvas, top: f32, size: f32, centres: &[f32], rows: &[Vec<String>]) {
    for (index, row) in rows.iter().enumerate() {
        let baseline = (index as f32).mul_add(ROW_PITCH, top + ROW_BASELINE);
        for (text, &centre) in row.iter().zip(centres) {
            let fitted = shrink_to_fit(text, Font::Regular, size, 6.0, cell_room(centre));
            canvas.text_centred(
                Font::Regular,
                fitted,
                centre.mul_add(LEFT_WIDTH, MARGIN),
                baseline,
                text,
            );
        }
    }
    let middle = MARGIN + LEFT_WIDTH / 2.0;
    let bottom = (rows.len() as f32).mul_add(ROW_PITCH, top);
    canvas.line((middle, top), (middle, bottom), THIN);
}

/// The left column of the template. Returns its bottom, where the views grid ends too.
fn draw_left_column(canvas: &mut Canvas, layout: &SheetLayout) -> f32 {
    let centre = MARGIN + LEFT_WIDTH / 2.0;
    let right = MARGIN + LEFT_WIDTH;
    let mut y = MARGIN;

    // The title: up to two lines at 18 pt, else smaller.
    let heading = layout.heading();
    let size = [18.0, 14.0, 12.0]
        .into_iter()
        .find(|&size| wrap(heading, Font::Bold, size, LEFT_WIDTH).len() <= 2)
        .unwrap_or(12.0);
    let lines = wrap(heading, Font::Bold, size, LEFT_WIDTH);
    let leading = size * 1.15;
    let text_height = leading * lines.len() as f32;
    let block = (text_height + 14.0).max(41.0);
    let first = size.mul_add(0.85, (block - text_height).mul_add(0.5, y));
    for (index, line) in lines.iter().enumerate() {
        let baseline = (index as f32).mul_add(leading, first);
        canvas.text_centred(Font::Bold, size, centre, baseline, line);
    }
    y += block;
    canvas.rule(MARGIN, right, y, THICK);
    y += THICK / 2.0;

    // The subtitle, a thin rule, then Author | Month and year.
    let details = &layout.details;
    let subtitle = details.subtitle.trim();
    let subtitle_size = shrink_to_fit(subtitle, Font::Regular, 11.0, 7.0, LEFT_WIDTH - 8.0);
    canvas.text_centred(Font::Regular, subtitle_size, centre, y + 17.0, subtitle);
    y += 26.5;
    canvas.rule(MARGIN, right, y, THIN);
    y += THIN / 2.0;
    for (text, fraction) in [
        (details.author.trim(), 0.25f32),
        (details.date_text.trim(), 0.75),
    ] {
        let fitted = shrink_to_fit(text, Font::Regular, 11.0, 7.0, LEFT_WIDTH / 2.0 - 6.0);
        canvas.text_centred(
            Font::Regular,
            fitted,
            fraction.mul_add(LEFT_WIDTH, MARGIN),
            y + 10.5,
            text,
        );
    }
    canvas.line((centre, y), (centre, y + 14.0), THIN);
    y += 14.0;

    // Facet Data: facets on the left half, tiers on the right.
    let top = block_heading(canvas, "Facet Data", MARGIN, LEFT_WIDTH, y);
    let rows: Vec<Vec<String>> = layout
        .facets
        .template_rows()
        .into_iter()
        .map(|(label, facets, tier_label, tiers)| {
            vec![label.to_string(), facets, tier_label.to_string(), tiers]
        })
        .collect();
    block_rows(canvas, top, 8.0, &[0.17, 0.42, 0.69, 0.94], &rows);
    y = (rows.len() as f32).mul_add(ROW_PITCH, top);

    // Size Data: three ratios on each half.
    let top = block_heading(canvas, "Size Data", MARGIN, LEFT_WIDTH, y);
    let cells = layout.sizes.cells();
    let rows: Vec<Vec<String>> = (0..3)
        .map(|i| {
            vec![
                cells[i].0.to_string(),
                cells[i].1.clone(),
                cells[i + 3].0.to_string(),
                cells[i + 3].1.clone(),
            ]
        })
        .collect();
    block_rows(canvas, top, 10.0, &[0.085, 0.335, 0.665, 0.915], &rows);
    y = (rows.len() as f32).mul_add(ROW_PITCH, top);

    // Design Data: label | value.
    let top = block_heading(canvas, "Design Data", MARGIN, LEFT_WIDTH, y);
    let rows: Vec<Vec<String>> = layout
        .design_data
        .iter()
        .map(|(label, value)| vec![label.clone(), value.clone()])
        .collect();
    block_rows(canvas, top, 10.0, &[0.25, 0.75], &rows);
    (rows.len() as f32).mul_add(ROW_PITCH, top)
}

/// The 2 x 2 grid of the four views beside the left column, from the top margin to `bottom`.
fn draw_views(canvas: &mut Canvas, views: &[VectorView; 4], bottom: f32) {
    let (x, y) = (VIEWS_X, MARGIN);
    let (width, height) = (VIEWS_WIDTH, bottom - MARGIN);
    let (half_width, half_height) = (width / 2.0, height / 2.0);
    // The frame and the two dividers.
    for (from, to) in [
        ((x, y), (x + width, y)),
        ((x, y + height), (x + width, y + height)),
        ((x, y), (x, y + height)),
        ((x + width, y), (x + width, y + height)),
        ((x + half_width, y), (x + half_width, y + height)),
        ((x, y + half_height), (x + width, y + half_height)),
    ] {
        canvas.line(from, to, GRID);
    }
    for (index, view) in views.iter().enumerate() {
        let column = (index % 2) as f32;
        let row = (index / 2) as f32;
        let cell = (
            column.mul_add(half_width, x),
            row.mul_add(half_height, y),
            half_width,
            half_height,
        );
        draw_vector_view(canvas, view, cell);
    }
}

/// `view` fitted into `cell` (`x`, `y`, width, height) with its aspect kept and a small inset:
/// its lines as vector paths (stroke widths scaled with the drawing), its labels and caption
/// as Helvetica text.
fn draw_vector_view(canvas: &mut Canvas, view: &VectorView, cell: (f32, f32, f32, f32)) {
    const INSET: f32 = 4.0;
    let (x, y, width, height) = cell;
    if view.width <= 0.0 || view.height <= 0.0 {
        return;
    }
    let scale =
        (INSET.mul_add(-2.0, width) / view.width).min(INSET.mul_add(-2.0, height) / view.height);
    if scale <= 0.0 {
        return;
    }
    let left = view.width.mul_add(-scale, width).mul_add(0.5, x);
    let top = view.height.mul_add(-scale, height).mul_add(0.5, y);
    let map = |(px, py): (f32, f32)| (px.mul_add(scale, left), py.mul_add(scale, top));

    canvas.content.set_line_cap(LineCapStyle::RoundCap);
    for weight in [LineWeight::Tick, LineWeight::Edge, LineWeight::Outline] {
        let mut any = false;
        for line in view.lines.iter().filter(|line| line.weight == weight) {
            if !any {
                canvas
                    .content
                    .set_line_width(view.stroke_width(weight) * scale);
                any = true;
            }
            let (from, to) = (map(line.from), map(line.to));
            canvas.content.move_to(from.0, Canvas::flip(from.1));
            canvas.content.line_to(to.0, Canvas::flip(to.1));
        }
        if any {
            canvas.content.stroke();
        }
    }
    canvas.content.set_line_cap(LineCapStyle::ButtCap);

    for label in view
        .labels
        .iter()
        .chain(std::iter::once(&view.caption_label()))
    {
        let size = label.size * scale;
        let (centre, middle) = map((label.x, label.y));
        canvas.text_centred(
            Font::Regular,
            size,
            centre,
            size.mul_add(BASELINE_DROP_EMS, middle),
            &label.text,
        );
    }
}

/// Comments: the heading over the left column's width, then every comment line wrapped and
/// centred across the page. Returns where the next block starts; nothing is drawn without
/// comments.
fn draw_comments(canvas: &mut Canvas, comments: &[String], y: f32) -> f32 {
    const SIZE: f32 = 9.0;
    const PITCH: f32 = 10.8;
    if comments.is_empty() {
        return y;
    }
    let mut y = block_heading(canvas, "Comments", MARGIN, LEFT_WIDTH, y);
    for comment in comments {
        for line in wrap(comment, Font::Regular, SIZE, CONTENT_WIDTH) {
            if y + PITCH > BOTTOM {
                canvas.new_page();
                y = MARGIN;
            }
            canvas.text_centred(Font::Regular, SIZE, PAGE_WIDTH / 2.0, y + 9.0, &line);
            y += PITCH;
        }
    }
    y
}

/// One section line, measured: its columns' text and the size each is set at.
struct MeasuredLine<'a> {
    line: &'a SectionLine,
    code_size: f32,
    index_size: f32,
    indices: Vec<String>,
    instruction: Vec<String>,
}

impl MeasuredLine<'_> {
    /// The row's height: one row pitch, plus a line pitch for every wrapped line.
    fn height(&self) -> f32 {
        let lines = self.indices.len().max(self.instruction.len()).max(1);
        ((lines - 1) as f32).mul_add(LINE_PITCH, ROW_PITCH)
    }
}

/// The height of a tier's lines together.
fn group_height(group: &[MeasuredLine<'_>]) -> f32 {
    group.iter().map(MeasuredLine::height).sum()
}

/// `line` measured for the section's columns: the code shrunk to its column, the index list
/// shrunk to [`MIN_INDEX_FONT`] and then wrapped after a dash, the instruction wrapped.
fn measure(line: &SectionLine) -> MeasuredLine<'_> {
    let code_size = shrink_to_fit(&line.code, Font::Regular, ROW_FONT, 6.0, CODE_WIDTH);
    let index_size = shrink_to_fit(
        &line.indices,
        Font::Regular,
        ROW_FONT,
        MIN_INDEX_FONT,
        INDEX_WIDTH,
    );
    MeasuredLine {
        line,
        code_size,
        index_size,
        indices: wrap(&line.indices, Font::Regular, index_size, INDEX_WIDTH),
        instruction: wrap(
            &line.instruction,
            Font::Regular,
            ROW_FONT,
            INSTRUCTION_WIDTH,
        ),
    }
}

/// One measured row with its top at `top`: the band (when shaded), then the four columns.
fn draw_row(canvas: &mut Canvas, row: &MeasuredLine<'_>, top: f32) {
    if row.line.shaded {
        canvas.shade(MARGIN, top, CONTENT_WIDTH, row.height());
    }
    let baseline = top + ROW_BASELINE;
    canvas.text_centred(
        Font::Regular,
        row.code_size,
        MARGIN + CODE_CENTRE,
        baseline,
        &row.line.code,
    );
    canvas.text_right(
        Font::Regular,
        ROW_FONT,
        MARGIN + ANGLE_RIGHT,
        baseline,
        &row.line.angle,
    );
    for (index, text) in row.indices.iter().enumerate() {
        canvas.text(
            Font::Regular,
            row.index_size,
            MARGIN + INDEX_LEFT,
            (index as f32).mul_add(LINE_PITCH, baseline),
            text,
        );
    }
    for (index, text) in row.instruction.iter().enumerate() {
        canvas.text(
            Font::Regular,
            ROW_FONT,
            MARGIN + INSTRUCTION_LEFT,
            (index as f32).mul_add(LINE_PITCH, baseline),
            text,
        );
    }
}

/// A section's heading at `y` (24 pt bold with a short thick underline). Returns the top of
/// its first row.
fn section_heading(canvas: &mut Canvas, heading: &str, y: f32) -> f32 {
    const SIZE: f32 = 24.0;
    let baseline = y + 38.0;
    canvas.text(Font::Bold, SIZE, MARGIN + 9.0, baseline, heading);
    let underline = (text_width(heading, Font::Bold, SIZE) + 18.0).max(77.0);
    let rule = baseline + 8.5;
    canvas.rule(MARGIN, MARGIN + underline, rule, THICK);
    rule + THICK / 2.0
}

/// One section (`Pavilion` or `Crown`) from `y`: its heading, then its lines one tier at a time
/// (a concave tier's facet and tool lines together), continuing on a new page with the heading
/// and "(continued)" when a tier does not fit. Returns where the next block starts; nothing is
/// drawn for a section with no lines.
fn draw_section(canvas: &mut Canvas, heading: &str, lines: &[SectionLine], y: f32) -> f32 {
    if lines.is_empty() {
        return y;
    }
    // One group per tier: a flat tier's line, or a concave tier's facet line and tool line.
    let mut groups: Vec<Vec<MeasuredLine<'_>>> = Vec::new();
    for line in lines {
        let measured = measure(line);
        if line.tool
            && let Some(group) = groups.last_mut()
        {
            group.push(measured);
        } else {
            groups.push(vec![measured]);
        }
    }

    // The heading never stands alone at the foot of a page.
    let first = groups.first().map_or(0.0, |group| group_height(group));
    let mut y = y;
    if y + 48.0 + first > BOTTOM {
        canvas.new_page();
        y = MARGIN;
    }
    let mut top = section_heading(canvas, heading, y);
    for group in &groups {
        if top + group_height(group) > BOTTOM {
            canvas.new_page();
            top = section_heading(canvas, &format!("{heading} (continued)"), MARGIN);
        }
        for row in group {
            draw_row(canvas, row, top);
            top += row.height();
        }
    }
    top
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut_sheet::{SheetDetails, tests::round_brilliant_design};

    /// Whether `bytes` contains `needle`.
    fn contains(bytes: &[u8], needle: &[u8]) -> bool {
        bytes.windows(needle.len()).any(|window| window == needle)
    }

    /// A section that starts near the foot of a page keeps its heading with its first tier, and
    /// the rest continues on a new page under the heading and "(continued)".
    #[test]
    fn a_section_that_does_not_fit_continues_under_its_heading() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
        let lines = layout.pavilion_lines();
        let mut canvas = Canvas::new();
        // Room for the heading and one tier only.
        let end = draw_section(&mut canvas, "Pavilion", &lines, BOTTOM - 70.0);
        assert!(end < BOTTOM);
        let pages = canvas.pages();
        assert_eq!(pages.len(), 2);
        let mut pages = pages.into_iter();
        let first = pages.next().expect("the first page").finish();
        let second = pages.next().expect("the second page").finish();
        assert!(contains(&first, b"(Pavilion)"));
        // pdf-writer escapes parentheses only when they are unbalanced.
        assert!(
            contains(&second, b"(Pavilion (continued))")
                || contains(&second, b"(Pavilion \\(continued\\))"),
            "{}",
            String::from_utf8_lossy(&second)
        );
    }

    /// A heading with no room for its first tier moves to the next page whole.
    #[test]
    fn a_heading_never_stands_alone_at_the_foot_of_a_page() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("every tier is pinned");
        let layout = SheetLayout::build(&design, &solved, &[], &SheetDetails::default());
        let mut canvas = Canvas::new();
        draw_section(&mut canvas, "Crown", &layout.crown_lines(), BOTTOM - 30.0);
        let pages = canvas.pages();
        assert_eq!(pages.len(), 2);
        let first = pages.into_iter().next().expect("the first page").finish();
        assert!(!contains(&first, b"(Crown"), "the first page stays empty");
    }

    #[test]
    fn index_lists_break_after_a_dash_and_words_after_a_space() {
        assert_eq!(break_units("96-24-48"), ["96-", "24-", "48"]);
        assert_eq!(break_units("Meet P1, P2"), ["Meet ", "P1, ", "P2"]);
        let wrapped = wrap(
            "95-01-11-13-23-25-35-37-47-49-59-61-71-73-83-85",
            Font::Regular,
            10.0,
            120.0,
        );
        assert!(wrapped.len() > 1, "{wrapped:?}");
        assert!(wrapped.iter().all(|line| !line.starts_with('-')));
        assert_eq!(
            wrapped.concat(),
            "95-01-11-13-23-25-35-37-47-49-59-61-71-73-83-85"
        );
        assert_eq!(wrap("", Font::Regular, 10.0, 100.0), [""]);
    }

    #[test]
    fn a_long_index_list_shrinks_before_it_wraps() {
        // Sixteen two-digit positions are about 228 pt at 10 pt, wider than the column.
        let list = "95-01-11-13-23-25-35-37-47-49-59-61-71-73-83-85";
        let size = shrink_to_fit(list, Font::Regular, ROW_FONT, MIN_INDEX_FONT, INDEX_WIDTH);
        assert!((MIN_INDEX_FONT..ROW_FONT).contains(&size), "{size}");
        assert_eq!(wrap(list, Font::Regular, size, INDEX_WIDTH).len(), 1);
    }
}
