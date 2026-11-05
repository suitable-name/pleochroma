//! A design's printable cutting instructions as a single self-contained HTML string or a PDF.
//!
//! Printable HTML that prints straight from the browser, the same sheet as a PDF, and the
//! standalone 2D faceting diagram (PNG bytes) that shares their rendering pipeline.
//!
//! Everything here is a pure function of `&Design` + `&[SolvedTier]` (+ the caller's
//! [`SheetDetails`]): [`render_cut_diagram`] and [`render_cut_views`] (mesh ->
//! `pleochroma_solid::diagram2d` raster -> PNG bytes in memory), [`render_vector_cut_views`]
//! (the same views as line drawings), [`cutting_sheet_document_with`] (one HTML string) and
//! [`cutting_sheet_pdf_with`] (PDF bytes), both written from [`SheetLayout`], which is built from
//! [`pleochroma_cut_core::Design::cutting_sheet`]'s own rows. Nothing touches a file system,
//! dialog or clock: the desktop writes the bytes to a picked path, the web app hands them to a
//! browser download, and the export date is text the caller supplies.
//!
//! # Layout
//!
//! The printed sheet follows the official cutting-instructions template: a left column with the
//! title, subtitle, Author | Month and year, Facet Data, Size Data and Design Data; beside it a
//! 2 x 2 grid of the four views as line drawings (`down +Z`, `down +X`, `down -Y`, `down -Z`);
//! then Comments, a Pavilion and a Crown section. The HTML and the PDF draw the same template
//! (US Letter); the HTML adds an "Additional information" section after it, on a page of its
//! own, with the shaded renderings at print resolution, a Stone summary ([`StoneSummary`]: the
//! size in millimetres, the proportions, the material and the carat estimate with its scaling),
//! the Orientation block and a mast table. On screen the HTML shows each printed page as a white
//! sheet on grey under a screen-only bar; printed, it is the plain Letter pages. The counts,
//! ratios and rows are computed once in [`SheetLayout`]; the HTML, the PDF and the text sheet are
//! all written from it.
//!
//! # Pavilion/Crown sections
//!
//! A row's section comes from the solver's own `classify_blocks`
//! (`design.meet_tier_inputs()` in, one `Block` per tier out, aligned with the cutting
//! sheet's one-row-per-tier order) -- never derived from the sign of a formatted angle
//! string. The girdle rows are part of the Pavilion section. An index list prints index 0 as
//! the gear's tooth count (`96-24-48-72`), as the template does.
//!
//! # Model-unit vs. mm masts
//!
//! The mast table's "Mast (mm)" column only appears when
//! [`pleochroma_cut_core::Design::girdle_diameter_mm`] is set -- not when the mm
//! conversion actually resolves. A design with a stated girdle diameter that
//! currently fails to close still shows the column, with `"-"` cells, rather than
//! silently reverting to a model-units-only table.

mod details;
mod diagram;
mod html;
mod layout;
mod orientation;
mod pdf;
mod pdf_font;
mod summary;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_layout;
#[cfg(test)]
mod tests_pdf;

pub use details::{SheetDetails, month_year_text};
pub use diagram::{
    CutViews, DiagramImage, render_cut_diagram, render_cut_views, render_vector_cut_views,
};
pub use html::{cutting_sheet_html, cutting_sheet_html_with, cutting_sheet_html_with_default};
pub use layout::{
    BlockCount, FacetData, NO_VALUE, SectionLine, SheetLayout, SizeData, UNTITLED_HEADING,
    template_instruction,
};
pub use orientation::{AxisDiagram, AxisLine, axis_diagram};
pub use summary::{StoneSummary, SummaryRow};

/// Three-panel diagram resolution of the standalone [`render_cut_diagram`] embed.
///
/// The printed sheet itself no longer embeds this picture: its template shows the four line
/// drawings, and its additional information the four shaded views of [`SHEET_VIEW_WIDTH`] x
/// [`SHEET_VIEW_HEIGHT`].
pub const SHEET_DIAGRAM_WIDTH: u32 = 900;
/// See [`SHEET_DIAGRAM_WIDTH`].
pub const SHEET_DIAGRAM_HEIGHT: u32 = 360;

/// Resolution of each of the four shaded renderings in the HTML sheet's additional information.
///
/// Print resolution: two views per row of a 7.7 in column, so each prints about 3.8 in wide at
/// over 400 dpi. Larger than the template's line drawings need, since these are the pictures a
/// cutter zooms into.
pub const SHEET_VIEW_WIDTH: u32 = 1600;
/// See [`SHEET_VIEW_WIDTH`].
pub const SHEET_VIEW_HEIGHT: u32 = 1257;

/// Drawing size, in drawing units, of each line-drawing view of the template.
///
/// It has the aspect of the template's grid cells ([`render_vector_cut_views`]). The units are
/// the raster layout's pixels (a frame this size lays out exactly as the line drawing), and the
/// HTML and PDF scale each drawing into its cell.
pub const VECTOR_VIEW_WIDTH: u32 = 560;
/// See [`VECTOR_VIEW_WIDTH`].
pub const VECTOR_VIEW_HEIGHT: u32 = 600;

/// The standalone diagram export's resolution -- print-quality (roughly 150 DPI at
/// A4 landscape width) rather than the cutting sheet's own smaller embed.
pub const DIAGRAM_EXPORT_WIDTH: u32 = 1800;
/// See [`DIAGRAM_EXPORT_WIDTH`].
pub const DIAGRAM_EXPORT_HEIGHT: u32 = 720;

/// The message a sheet or diagram export gives for a design that does not close to a solid.
const NOT_CLOSED: &str = "This design does not currently close to a solid -- nothing to draw.";

/// The complete cutting instructions as the HTML string a caller writes or downloads.
///
/// With the four views when the design currently closes, and the header words from
/// [`SheetDetails::from_design`]: exactly [`cutting_sheet_document_with`] with those details.
/// `custom` is threaded through to the printed refractive index.
///
/// # Panics
///
/// Same alignment contract as [`pleochroma_cut_core::Design::cutting_sheet`]:
/// `solved` must have one entry per tier `design` currently has, in the same order.
#[must_use]
pub fn cutting_sheet_document(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
) -> String {
    cutting_sheet_document_with(design, solved, custom, &SheetDetails::from_design(design))
}

/// [`cutting_sheet_document`] with explicit header `details` (title, author, date, shape,
/// comments, ranges). Pure: it reads no clock, so the date is whatever `details.date_text`
/// says.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
#[must_use]
pub fn cutting_sheet_document_with(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
    details: &SheetDetails,
) -> String {
    cutting_sheet_document_with_default(design, solved, custom, None, details)
}

/// [`cutting_sheet_document_with`] with the stored default orientation of the design's material.
///
/// The orientation is `LibraryMaterial::default_orientation` of its library row, so a custom
/// material whose cutter set "tilt 0" still prints its Orientation block. With `None` it is
/// [`cutting_sheet_document_with`].
///
/// The template's line drawings and the shaded renderings of the additional information
/// ([`SHEET_VIEW_WIDTH`] x [`SHEET_VIEW_HEIGHT`]) are both drawn here.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
#[must_use]
pub fn cutting_sheet_document_with_default(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
    stored_default: Option<pleochroma_cut_core::CrystalOrientation>,
    details: &SheetDetails,
) -> String {
    let views = render_cut_views(design, solved, SHEET_VIEW_WIDTH, SHEET_VIEW_HEIGHT);
    cutting_sheet_html_with_default(
        design,
        solved,
        views.as_ref(),
        custom,
        stored_default,
        details,
    )
}

/// The complete cutting instructions as PDF bytes, with the header words from
/// [`SheetDetails::from_design`]: exactly [`cutting_sheet_pdf_with`] with those details.
///
/// # Errors
///
/// When `design` does not currently close to a solid: the template's four views would be
/// empty, so nothing is written rather than a sheet without its drawings.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
pub fn cutting_sheet_pdf(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
) -> Result<Vec<u8>, String> {
    cutting_sheet_pdf_with(design, solved, custom, &SheetDetails::from_design(design))
}

/// [`cutting_sheet_pdf`] with explicit header `details`. Pure: it reads no clock.
///
/// # Errors
///
/// See [`cutting_sheet_pdf`].
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
pub fn cutting_sheet_pdf_with(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
    details: &SheetDetails,
) -> Result<Vec<u8>, String> {
    cutting_sheet_pdf_with_default(design, solved, custom, None, details)
}

/// The cutting instructions as a PDF: the official template on US Letter pages.
///
/// It is drawn from the same [`SheetLayout`] as [`cutting_sheet_document_with_default`]'s main
/// part, the four views as vector line drawings, Helvetica text (the standard PDF fonts, nothing
/// embedded). The Pavilion and Crown lines continue on further pages when they do not fit on the
/// first.
///
/// The PDF holds the template only: the shaded renderings, the Orientation block and the mast
/// table are in the HTML sheet's additional information. Deterministic: no date and no document
/// ID, so the same design gives the same bytes.
///
/// # Errors
///
/// See [`cutting_sheet_pdf`].
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
pub fn cutting_sheet_pdf_with_default(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
    stored_default: Option<pleochroma_cut_core::CrystalOrientation>,
    details: &SheetDetails,
) -> Result<Vec<u8>, String> {
    let views = render_vector_cut_views(design, solved, VECTOR_VIEW_WIDTH, VECTOR_VIEW_HEIGHT)
        .ok_or_else(|| NOT_CLOSED.to_string())?;
    let layout = SheetLayout::build_with_default(design, solved, custom, stored_default, details);
    Ok(pdf::sheet_pdf(&layout, &views))
}

/// The cutting instructions as plain text, under plain headings.
///
/// The blocks are the HTML sheet's (header, Facet Data, Size Data, Design Data, comments,
/// Pavilion, Crown). The four views are pictures and appear only in the HTML and the PDF.
///
/// # Panics
///
/// Same alignment contract as [`cutting_sheet_document`].
#[must_use]
pub fn cutting_sheet_text_with(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
    custom: &[pleochroma::optics::materials::GemMaterial],
    details: &SheetDetails,
) -> String {
    SheetLayout::build(design, solved, custom, details).to_text()
}

/// The standalone diagram export: the 2D diagram alone at print resolution
/// ([`DIAGRAM_EXPORT_WIDTH`] x [`DIAGRAM_EXPORT_HEIGHT`]), PNG-encoded.
///
/// # Errors
///
/// When `design` does not currently close to a solid (nothing to diagram -- named
/// explicitly rather than silently producing a blank/stale image).
pub fn diagram_export_png(
    design: &pleochroma_cut_core::Design,
    solved: &[pleochroma::geometry::meet_solver::SolvedTier],
) -> Result<Vec<u8>, String> {
    render_cut_diagram(design, solved, DIAGRAM_EXPORT_WIDTH, DIAGRAM_EXPORT_HEIGHT)
        .map(|image| image.png_bytes)
        .ok_or_else(|| {
            "This design does not currently close to a solid -- nothing to diagram.".to_string()
        })
}
