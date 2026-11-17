//! Which stored file an entry opens from, and reading it as a [`Design`].
//!
//! The rule is the vault's: a self-contained `.psdd` design file first, else the first `.asc`,
//! else the first `.gem`, else the first `.gcs` (converted to `.asc`).
//! Other attachments (PDFs, images) are never a design.
//!
//! `.asc` has no notation for frosting, so the frosting of an entry read from one of those
//! comes from the rows the library stored with the entry (what the import of a `.gcs` or a
//! saved design file recorded), or from the `.gcs` itself; a design file states its own.

use pleochroma_cut_core::{
    Design,
    native::{DesignExtras, design_from_str, design_to_string},
};
use pleochroma_editor::{files::sanitize_file_name, loading::design_from_asc_text};
use pleochroma_formats::native::design::DESIGN_EXTENSION_DOTTED;
use pleochroma_library_mirror::browse::DesignFile;
use pleochroma_vault::{
    local::{
        DesignFileKind, design_attachment_position, design_file_to_asc_text, is_native_design_name,
        native_design_attachment_position,
    },
    model::{file::AttachedFile, frosting::StoredTierFrosting},
};
use pleochroma_web_core::input::MAX_DESIGN_FILE_BYTES;

use super::LibraryDesign;
use crate::cutting::progress_file_stems;

/// The file a design that was fetched from the computer opens from, the same shape
/// `LibraryHandle::load` gives for a copied design:
///
/// - a native design file as it came;
/// - an `.asc` as it came, when the design states no frosting;
/// - an `.asc` with frosting as a native design file named after `title`, because `.asc` cannot
///   state the frosting and the editor would lose it.
///
/// # Errors
/// A sentence when the frosted `.asc` cannot be read or written as a design file, or when the
/// file is over the 16 MiB a design file may have on this phone ("<title> is too large to open on
/// this phone.").
pub fn design_file_bytes(file: DesignFile, title: &str) -> Result<LibraryDesign, String> {
    let design = match file {
        DesignFile::Native { file_name, bytes } => LibraryDesign { file_name, bytes },
        DesignFile::Asc {
            file_name,
            text,
            frosting,
        } if frosting.is_empty() => LibraryDesign {
            file_name,
            bytes: text.into_bytes(),
        },
        DesignFile::Asc {
            file_name,
            text,
            frosting,
        } => {
            let stated = StoredTierFrosting::to_gcs(&frosting);
            let loaded = design_from_asc_text(&file_name, &text, None)
                .map_err(|error| format!("\"{file_name}\" cannot be read: {error}"))?;
            let mut design = loaded.design;
            design.apply_imported_frosting(&stated);
            let native =
                design_to_string(&design, None, &DesignExtras::default()).map_err(|error| {
                    format!("The design cannot be written as a design file: {error}")
                })?;
            let name = if title.trim().is_empty() {
                "Untitled"
            } else {
                title
            };
            LibraryDesign {
                file_name: format!("{}{DESIGN_EXTENSION_DOTTED}", sanitize_file_name(name)),
                bytes: native.into_bytes(),
            }
        }
    };
    if design.bytes.len() as u64 > MAX_DESIGN_FILE_BYTES {
        let name = if title.trim().is_empty() {
            "This design"
        } else {
            title
        };
        return Err(format!("{name} is too large to open on this phone."));
    }
    Ok(design)
}

/// The attachment of `files` an entry opens from, if there is one.
pub(super) fn primary_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    let names = || files.iter().map(|file| file.name.as_str());
    native_design_attachment_position(names())
        .or_else(|| design_attachment_position(names()).map(|(position, _)| position))
        .and_then(|position| files.get(position))
}

/// The design the attachments of one entry describe, polished: for a place that has no
/// frosting to apply (measuring a file before it is stored).
pub(super) fn design_of(files: &[AttachedFile]) -> Result<Design, String> {
    design_of_with_frosting(files, &[])
}

/// The design the attachments of one entry describe, with the `frosting` the library stored
/// for the entry applied when the design is read from `.asc` cutting instructions (a design
/// file carries its own).
pub(super) fn design_of_with_frosting(
    files: &[AttachedFile],
    frosting: &[StoredTierFrosting],
) -> Result<Design, String> {
    let attachment =
        primary_attachment(files).ok_or_else(|| "This entry holds no design file.".to_owned())?;
    design_from_attachment(attachment, frosting)
}

/// The design one stored file describes.
fn design_from_attachment(
    file: &AttachedFile,
    stored: &[StoredTierFrosting],
) -> Result<Design, String> {
    let name = &file.name;
    if is_native_design_name(name) {
        let text = std::str::from_utf8(&file.content)
            .map_err(|_| format!("\"{name}\" is not a text (UTF-8) design file."))?;
        return design_from_str(text)
            .map(|loaded| loaded.design)
            .map_err(|error| format!("\"{name}\" cannot be read: {error}"));
    }
    let kind = DesignFileKind::from_file_name(name)
        .ok_or_else(|| format!("\"{name}\" is not a design file."))?;
    let converted = design_file_to_asc_text(name, kind, &file.content)?;
    // The stored rows win; a `.gcs` read here states its own.
    let frosting = if stored.is_empty() {
        converted.frosting
    } else {
        StoredTierFrosting::to_gcs(stored)
    };
    design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map(|loaded| {
            let mut design = loaded.design;
            design.apply_imported_frosting(&frosting);
            design
        })
        .map_err(|error| format!("\"{name}\" cannot be read: {error}"))
}

/// The names of the done-mark files the design behind `files` and `frosting` keeps its
/// cutting progress in (see `cutting::progress_file_stems`). Empty when the entry holds no
/// readable design: there is then nothing to name.
///
/// The names are the ones the open design gets, because they are computed from the text
/// the editor opens: the stored native file itself, the `.asc` a `.gem` or `.gcs` import
/// converts to, or, for a frosted entry stored as `.asc`, the native text that
/// `LibraryHandle::load` completes it to.
pub(super) fn progress_stems(
    files: &[AttachedFile],
    frosting: &[StoredTierFrosting],
) -> Vec<String> {
    let Some(attachment) = primary_attachment(files) else {
        return Vec::new();
    };
    if is_native_design_name(&attachment.name) {
        return std::str::from_utf8(&attachment.content)
            .map_or_else(|_| Vec::new(), stems_of_native_text);
    }
    let Ok(design) = design_from_attachment(attachment, frosting) else {
        return Vec::new();
    };
    if frosting.is_empty() {
        return progress_file_stems("", &design);
    }
    design_to_string(&design, None, &DesignExtras::default())
        .map_or_else(|_| Vec::new(), |text| stems_of_native_text(&text))
}

/// The done-mark file names of the design a native design file's `text` holds.
fn stems_of_native_text(text: &str) -> Vec<String> {
    design_from_str(text).map_or_else(
        |_| Vec::new(),
        |loaded| progress_file_stems(&loaded.metadata.id, &loaded.design),
    )
}
