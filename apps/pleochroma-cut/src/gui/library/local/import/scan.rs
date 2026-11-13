//! Finds the files an import reads.
//!
//! Walks a folder for `.asc`, `.gem`, `.gcs` and stand-alone `.psdd` candidates, with an
//! optional guarded recursive descent. It also finds the design file beside a design.

use super::foreign::ForeignFormat;
use pleochroma_formats::native::design::{DESIGN_EXTENSION, FileKind, detect_kind, is_design_path};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tracing::warn;

/// Depth cap for the optional subfolder recursion.
///
/// Symlink and junction loops are caught by the `visited` set in
/// [`collect_design_files_recursive`]. This only guards an absurdly deep tree without a cycle.
pub(super) const MAX_RECURSE_DEPTH: usize = 32;

/// The kinds a `.psdd` design file attaches to, in the order the library reads them.
///
/// `.asc` comes first, then `.gem` and `.gcs`. Both letter cases are tried, so the match also
/// works on a case-sensitive file system.
const SCHEDULE_EXTENSIONS: [&str; 6] = ["asc", "ASC", "gem", "GEM", "gcs", "GCS"];

/// The `.asc`/`.gem`/`.gcs` file beside the design file at `design_path`, if one exists.
///
/// A design file with such a sibling is imported attached to it, not as a design of its own.
#[must_use]
fn schedule_sibling(design_path: &Path) -> Option<PathBuf> {
    SCHEDULE_EXTENSIONS
        .iter()
        .map(|extension| design_path.with_extension(extension))
        .find(|candidate| candidate.is_file())
}

/// `true` for a file that Import reads as a design of its own.
///
/// These are `.asc` files, `GemCAD` `.gem` files, Gem Cut Studio `.gcs` files (extension
/// compared case-insensitively) and stand-alone `.psdd` files. The `.gem`/`.gcs` files are
/// converted on import (see [`super::foreign`]). A `.psdd` with one of those beside it is
/// attached to that design instead ([`find_design_file_beside`]).
#[must_use]
fn is_importable_design(path: &Path) -> bool {
    if is_design_path(path) {
        return schedule_sibling(path).is_none();
    }
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
        || ForeignFormat::from_path(path).is_some()
}

/// Walks one directory for importable design files ([`is_importable_design`]),
/// descending into subdirectories when `recurse` is true.
///
/// `visited` holds the canonicalized (symlink-resolved) path of every directory already
/// entered. Comparing canonical paths catches a symlink or junction that loops back to an
/// ancestor, which raw path text would miss. `depth` is capped at `max_depth` as a second,
/// independent guard. Both guards skip the directory and log via `warn!`, so one bad
/// subfolder does not hide design files elsewhere in the tree.
pub(super) fn collect_design_files_recursive(
    dir: &Path,
    recurse: bool,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth {
        warn!(
            "Import: not descending into '{}' -- exceeded max recursion depth ({max_depth})",
            dir.display()
        );
        return;
    }
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canon) {
        warn!(
            "Import: skipping '{}' -- already visited (symlink loop?)",
            dir.display()
        );
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && is_importable_design(&p) {
            out.push(p);
        } else if recurse && p.is_dir() {
            collect_design_files_recursive(&p, recurse, depth + 1, max_depth, visited, out);
        }
    }
}

/// Resolves what [`super::pipeline::import_path`] imports.
///
/// That is the single file `path` names, or every design file in a folder, plus its
/// subfolders when `recurse` (see [`collect_design_files_recursive`]).
///
/// A picked design file with a `.asc`/`.gem`/`.gcs` beside it stands for that design, so
/// the design file is attached to it; without one it is imported as a design of its own.
///
/// `Err` carries the user-facing message `import_path` returns verbatim: an unreadable
/// folder, a path that is neither file nor folder, or a folder with no design file. Split
/// out of `import_path` for clippy's `too_many_lines` limit.
pub(super) fn collect_import_candidates(
    path: &Path,
    recurse: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                // The picked folder itself is not put in `visited`; only directories
                // walked below it are guarded.
                let mut visited: HashSet<PathBuf> = HashSet::new();
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && is_importable_design(&p) {
                        candidates.push(p);
                    } else if recurse && p.is_dir() {
                        collect_design_files_recursive(
                            &p,
                            recurse,
                            1,
                            MAX_RECURSE_DEPTH,
                            &mut visited,
                            &mut candidates,
                        );
                    }
                }
            }
            Err(e) => return Err(format!("Could not read folder '{}': {e}", path.display())),
        }
    } else if path.is_file() {
        if is_design_path(path) {
            candidates.push(schedule_sibling(path).unwrap_or_else(|| path.to_path_buf()));
        } else {
            candidates.push(path.to_path_buf());
        }
    } else {
        return Err(format!("'{}' is not a file or folder.", path.display()));
    }

    if candidates.is_empty() {
        return Err(format!(
            "No .asc, .gem, .gcs or .psdd files found at '{}'.",
            path.display()
        ));
    }
    Ok(candidates)
}

/// Reads `path` as a file to attach: its bare name and bytes.
fn read_attachment(path: &Path) -> Option<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((name, bytes))
}

/// Reads the `<stem>.psdd` design file beside `path`, a `.asc`, `.gem` or `.gcs`.
///
/// Returns `None` when there is no such file, or it is not a design file. Attaching something
/// the editor cannot open would only hide the `.asc` behind it. The editor prefers this file
/// over the schedule it is attached to, since it carries the whole design.
pub(super) fn find_design_file_beside(path: &Path) -> Option<(String, Vec<u8>)> {
    let (name, bytes) = read_attachment(&path.with_extension(DESIGN_EXTENSION))?;
    (detect_kind(&bytes) == FileKind::Design).then_some((name, bytes))
}
