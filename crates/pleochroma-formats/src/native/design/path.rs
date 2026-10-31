//! File-name rules and media-type constants for design files. Pure path arithmetic,
//! no filesystem access.
//!
//! Every save writes `.psdd` (Pleochroma Suite Design Document), and `.psdd` is the only
//! design file extension that is read.

use super::codec::FileKind;
use std::path::{Path, PathBuf};

/// The design file extension every save writes, without the leading dot.
pub const DESIGN_EXTENSION: &str = "psdd";

/// [`DESIGN_EXTENSION`] with its leading dot, as a file picker filter or a file name
/// suffix wants it.
pub const DESIGN_EXTENSION_DOTTED: &str = ".psdd";

/// Every extension that names a design file when READING (file dialog filters, drop
/// and scan accept lists).
pub const DESIGN_READ_EXTENSIONS: [&str; 1] = [DESIGN_EXTENSION];

/// The media type of a design file (TOML text under a vendor type).
pub const DESIGN_MIME_TYPE: &str = "application/vnd.pleochroma.design+toml";

/// An HTML `accept` attribute value that admits a design file by either extension or
/// by media type.
pub const DESIGN_ACCEPT: &str = ".psdd,application/vnd.pleochroma.design+toml";

/// The human-readable name of the file type, for dialogs. It carries no extension: a
/// filter label appends one, e.g. `"Pleochroma design file (*.psdd)"`.
pub const DESIGN_FILE_DESCRIPTION: &str = "Pleochroma design file";

/// `true` iff `path` names a design file: its last extension is `psdd`
/// (case-insensitive).
#[must_use]
pub fn is_design_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            DESIGN_READ_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// The design file name for a design called `stem`: `stem` -> `stem.psdd`.
#[must_use]
pub fn design_path_for(stem: &str) -> PathBuf {
    PathBuf::from(format!("{stem}{DESIGN_EXTENSION_DOTTED}"))
}

/// The design file path next to an existing file: `dir/foo.asc` gives `dir/foo.psdd`.
///
/// `None` when `path` has no file stem.
#[must_use]
pub fn design_path_for_sibling(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    Some(path.with_file_name(format!("{stem}{DESIGN_EXTENSION_DOTTED}")))
}

/// Tells a design file from anything else by its file name alone.
///
/// `.psdd` is a [`FileKind::Design`]; any other name is
/// [`FileKind::Unknown`]. The name is only a hint: [`super::detect_kind`] judges the
/// content.
#[must_use]
pub fn detect_kind_of_path(path: &Path) -> FileKind {
    if is_design_path(path) {
        FileKind::Design
    } else {
        FileKind::Unknown
    }
}
