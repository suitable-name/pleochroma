//! File pickers off the UI thread. Every save and open dialog of this module builds a
//! [`PickKind`] and passes a continuation to [`pick_file`].

use super::design_paths::design_file_name_for;
use crate::{MainWindow, gui::editor::state::EditorState};
use pleochroma_formats::native::design::{
    DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED, DESIGN_FILE_DESCRIPTION, DESIGN_READ_EXTENSIONS,
};
use std::path::{Path, PathBuf};

// --- Group 3: file pickers off the UI thread --------------------------------
//
// The picker machinery lives in `gui::pickers`, the one place the `rfd` file dialog is
// built. `PickKind`/`pick_file` translate into its `PickerRequest`.

/// Which native picker [`pick_file`] should show -- one entry per save/open
/// dialog this module's own write/export/open paths need, translated to a
/// [`crate::gui::pickers::PickerRequest`] by [`pick_file`] itself.
pub(super) enum PickKind {
    /// "Save" / "Save As...": the `.psdd` design-file save-as picker.
    SaveDesign { default_name: String },
    /// "Export Edited .asc": the `.asc` save-as picker.
    SaveAsc { default_name: String },
    /// "Export as Gem Cut Studio (.gcs)...": the `.gcs` save-as picker.
    SaveGcs { default_name: String },
    /// "Export Cutting Sheet (HTML)...": the `.html` save-as picker.
    SaveCuttingSheet { default_name: String },
    /// "Export Cutting Sheet (PDF)...": the `.pdf` save-as picker.
    SaveCuttingSheetPdf { default_name: String },
    /// "Export Diagram": the `.png` save-as picker.
    SaveDiagram { default_name: String },
    /// "Open": accepts a `.psdd` design file, a bare `.asc`, or a `.gem`/`.gcs` design
    /// (converted to `.asc` on open).
    OpenDesign,
}

/// The Open dialog's design-file filter label, listing every readable extension:
/// "Pleochroma design file (*.psdd)".
fn open_design_filter_label() -> String {
    let patterns: Vec<String> = DESIGN_READ_EXTENSIONS
        .iter()
        .map(|extension| format!("*.{extension}"))
        .collect();
    format!("{DESIGN_FILE_DESCRIPTION} ({})", patterns.join(", "))
}

/// The `.asc` filter shared by the dialogs that open or save `.asc` files. A `fn` because
/// [`PickerFilter`] owns its `String`s, which a `const` cannot build.
fn asc_filter() -> crate::gui::pickers::PickerFilter {
    crate::gui::pickers::PickerFilter {
        label: ".asc Design".to_string(),
        extensions: vec!["asc".to_string()],
    }
}

/// Translates `kind` into a [`crate::gui::pickers::PickerRequest`] and shows
/// it via [`crate::gui::pickers::pick`] -- the one entry point every native
/// file picker in this module uses. `state` must never be borrowed across a
/// call to this function -- every call site below picks first, then borrows,
/// never the other way around.
pub(super) fn pick_file(
    ui: &MainWindow,
    kind: PickKind,
    on_done: impl FnOnce(&MainWindow, Option<PathBuf>) + 'static,
) {
    use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick};

    let request = match kind {
        PickKind::SaveDesign { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: format!("{DESIGN_FILE_DESCRIPTION} (*{DESIGN_EXTENSION_DOTTED})"),
                extensions: vec![DESIGN_EXTENSION.to_string()],
            }],
            default_file_name: Some(default_name),
            // Where the design was last opened from, else the `./exports` convention.
            starting_dir: super::SUGGESTED_SAVE_DIR
                .with(|cell| cell.borrow().clone())
                .or_else(default_export_dir),
        },
        PickKind::SaveAsc { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![asc_filter()],
            default_file_name: Some(default_name),
            // Starts in `./exports` when it exists, as the library's export paths do.
            starting_dir: default_export_dir(),
        },
        PickKind::SaveGcs { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Gem Cut Studio Design (.gcs)".to_string(),
                extensions: vec!["gcs".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: default_export_dir(),
        },
        // Each cutting-sheet item offers its own file type only. The written format still
        // follows the picked name (`cut_sheet::write_cutting_sheet_with`): a name typed with
        // `.pdf` in the HTML dialog writes the PDF.
        PickKind::SaveCuttingSheet { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Cutting Sheet (HTML)".to_string(),
                extensions: vec!["html".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        PickKind::SaveCuttingSheetPdf { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Cutting Sheet (PDF)".to_string(),
                extensions: vec!["pdf".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        PickKind::SaveDiagram { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Diagram (PNG)".to_string(),
                extensions: vec!["png".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        PickKind::OpenDesign => PickerRequest {
            kind: PickerKind::OpenFile,
            title: Some("Open Design".to_string()),
            filters: vec![
                PickerFilter {
                    label: open_design_filter_label(),
                    extensions: DESIGN_READ_EXTENSIONS
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                },
                asc_filter(),
                PickerFilter {
                    label: "GemCAD / Gem Cut Studio Design (.gem, .gcs)".to_string(),
                    extensions: vec!["gem".to_string(), "gcs".to_string()],
                },
            ],
            default_file_name: None,
            starting_dir: None,
        },
    };
    pick(ui, request, on_done);
}

/// The file name an Export or Save dialog defaults to: the design's `asc_filename` when set,
/// else the sanitized first header line (the `GemCad` title convention), else
/// `"edited_design.asc"`. Shared by the Export and Save dialogs so they propose the same name.
pub(super) fn suggested_file_name(st: &EditorState) -> String {
    if let Some(name) = &st.asc_filename {
        return name.clone();
    }
    match st.design.meta.headers.first() {
        Some(header) if !header.trim().is_empty() => {
            format!(
                "{}.asc",
                crate::gui::library::local::sanitize_filename(header)
            )
        }
        _ => "edited_design.asc".to_string(),
    }
}

/// The file name a Save/Save As dialog should default to: the design's own name with
/// the `.psdd` extension (`foo.asc` -> `foo.psdd`). Derived from
/// [`suggested_file_name`], so the two dialogs stay in step apart from the extension.
pub(super) fn suggested_design_file_name(st: &EditorState) -> String {
    design_file_name_for(&suggested_file_name(st))
}

/// The `./exports` folder as a starting directory, when it exists (the manual's
/// `11-saving-and-file-formats.md` documents it). `None` otherwise, so the OS keeps its
/// last-used directory. See [`crate::gui::pickers::PickerRequest::starting_dir`].
fn default_export_dir() -> Option<PathBuf> {
    let default_dir = Path::new("exports");
    default_dir.is_dir().then(|| default_dir.to_path_buf())
}
