//! The "thin callback" half of the cutting-sheet/diagram exports: builds the bytes
//! through `pleochroma_editor::cut_sheet`, then does the plain file-system write
//! that has to live somewhere.

use pleochroma::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use pleochroma_cut_core::{CrystalOrientation, Design};
#[cfg(test)]
use pleochroma_editor::cut_sheet::cutting_sheet_document;
use pleochroma_editor::cut_sheet::{
    SheetDetails, cutting_sheet_document_with_default, cutting_sheet_pdf_with_default,
    diagram_export_png,
};
use std::path::{Path, PathBuf};

/// Which file the cutting-sheet export writes: the extension of the picked file decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetFormat {
    /// The self-contained web page: the template, then the additional information.
    Html,
    /// The PDF: the template on US Letter pages.
    Pdf,
}

impl SheetFormat {
    /// [`Self::Pdf`] for a file ending in `.pdf` (any case), else [`Self::Html`].
    #[must_use]
    pub fn of_path(path: &Path) -> Self {
        let pdf = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
        if pdf { Self::Pdf } else { Self::Html }
    }

    /// The path the export writes for a file picked as `path` by the save dialog of this format.
    ///
    /// For [`Self::Pdf`] a name that does not end in `.pdf` gets `.pdf` appended (a file dialog
    /// that does not add the extension of its only file type), so the PDF item never writes the
    /// HTML page. For [`Self::Html`] the path is returned unchanged: [`Self::of_path`] still lets
    /// a name ending in `.pdf` write the PDF.
    #[must_use]
    pub fn path_for(self, path: PathBuf) -> PathBuf {
        if self == Self::Pdf && Self::of_path(&path) != Self::Pdf {
            let mut name = path.into_os_string();
            name.push(".pdf");
            PathBuf::from(name)
        } else {
            path
        }
    }
}

/// Builds `design`/`solved`'s cutting sheet (with its four views, when the
/// design currently closes -- see `pleochroma_editor::cut_sheet::
/// cutting_sheet_document`) with the header words the design carries on its own
/// ([`SheetDetails::from_design`]) and writes it to `dest` as HTML.
///
/// Only the tests and identity pins write a sheet without the open design's file details;
/// the Export callback goes through [`write_cutting_sheet_with`]. `custom` is threaded
/// straight through to the sheet's "RI range" line.
///
/// # Errors
///
/// The write failure, ready to show as a toast.
#[cfg(test)]
pub fn write_cutting_sheet_html(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
    custom: &[GemMaterial],
) -> Result<(), String> {
    let html = cutting_sheet_document(design, solved, custom);
    write_bytes(dest, html.as_bytes())
}

/// Builds the cutting sheet with explicit header `details` (the open design's title,
/// designer, shape and notes, and the export date) and writes it to `dest`: as a PDF when
/// `dest` ends in `.pdf` ([`SheetFormat::of_path`]), else as the HTML page.
///
/// The save-as file-picker dialog is handled by `native_io`'s own job
/// (`native_io::pick_file`, on a background thread), so this stays callable with
/// no `EditorState`/UI dependency at all, and safely callable from a background
/// thread itself. The actual `std::fs::write` also belongs on a background thread.
///
/// `stored_default` is the stored default orientation of the design's custom material, read from
/// its library row (`None` for a built-in or a material that stores none).
///
/// # Errors
///
/// The write failure, or for a PDF a design that does not close to a solid (the template's
/// views would be empty), ready to show as a toast.
pub fn write_cutting_sheet_with(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
    custom: &[GemMaterial],
    stored_default: Option<CrystalOrientation>,
    details: &SheetDetails,
) -> Result<(), String> {
    match SheetFormat::of_path(dest) {
        SheetFormat::Html => {
            let html = cutting_sheet_document_with_default(
                design,
                solved,
                custom,
                stored_default,
                details,
            );
            write_bytes(dest, html.as_bytes())
        }
        SheetFormat::Pdf => {
            let pdf =
                cutting_sheet_pdf_with_default(design, solved, custom, stored_default, details)?;
            write_bytes(dest, &pdf)
        }
    }
}

/// Writes `bytes` to `dest`, creating its folder first.
fn write_bytes(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(dest, bytes).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

/// The "thin callback" half of the standalone diagram export: the 2D diagram alone,
/// at print resolution (`pleochroma_editor::cut_sheet::diagram_export_png`),
/// written to `dest`.
///
/// The save-as file-picker is handled by `native_io`, not by this function --
/// see [`write_cutting_sheet_with`]'s own doc comment.
///
/// # Errors
///
/// `Err` when `design` does not currently close to a solid (nothing to
/// diagram -- named explicitly rather than silently saving a blank/stale
/// image) or when the write itself fails.
pub fn write_diagram_png(
    design: &Design,
    solved: &[SolvedTier],
    dest: &Path,
) -> Result<(), String> {
    let png_bytes = diagram_export_png(design, solved)?;
    write_bytes(dest, &png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pdf_item_always_writes_a_pdf_name() {
        for (picked, written) in [
            ("sheet", "sheet.pdf"),
            ("sheet.pdf", "sheet.pdf"),
            ("sheet.PDF", "sheet.PDF"),
            ("sheet.html", "sheet.html.pdf"),
        ] {
            let path = SheetFormat::Pdf.path_for(PathBuf::from(picked));
            assert_eq!(path, PathBuf::from(written), "{picked}");
            assert_eq!(SheetFormat::of_path(&path), SheetFormat::Pdf, "{picked}");
        }
        // The HTML item keeps the picked name; the extension still decides the format.
        for picked in ["sheet", "sheet.html", "sheet.pdf"] {
            assert_eq!(
                SheetFormat::Html.path_for(PathBuf::from(picked)),
                PathBuf::from(picked)
            );
        }
    }

    #[test]
    fn the_extension_picks_the_sheet_format() {
        for (name, format) in [
            ("sheet.pdf", SheetFormat::Pdf),
            ("sheet.PDF", SheetFormat::Pdf),
            ("sheet.html", SheetFormat::Html),
            ("sheet.htm", SheetFormat::Html),
            ("sheet", SheetFormat::Html),
        ] {
            assert_eq!(SheetFormat::of_path(&PathBuf::from(name)), format, "{name}");
        }
    }
}
