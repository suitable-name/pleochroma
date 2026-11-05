//! A design's printable cutting sequence and its standalone 2D diagram, written to
//! disk. Everything that decides what the sheet and the diagram contain lives in
//! `pleochroma_editor::cut_sheet` (pure `&Design` + `&[SolvedTier]` -> HTML string /
//! PDF bytes / PNG bytes, shared with the web app); [`write`] is the desktop's thin
//! file-system half, called by `native_io`'s export callbacks after their save dialog (one
//! File-menu item and dialog per format, [`SheetFormat`]; the picked file's extension chooses
//! HTML or PDF), and [`date`] reads the clock for the month and year the sheet's header prints.

mod date;
mod write;

pub use date::current_date_text;
#[cfg(test)]
pub use write::write_cutting_sheet_html;
pub use write::{SheetFormat, write_cutting_sheet_with, write_diagram_png};
