//! The parse-measure-save pipeline for import candidates.
//!
//! A candidate is an `.asc` (with its optional `.psdd`), a `.gem`/`.gcs` converted to `.asc`,
//! or a stand-alone `.psdd`. Each becomes a saved catalogue row. [`import_path`] runs a batch.

use super::{
    measure::{apply_measured_metadata, merge_reimport_metadata},
    scan::{collect_import_candidates, find_design_file_beside},
};
use pleochroma_vault::{db::sqlite::Database, local};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};
use tracing::warn;

/// Runs `f`, turning a panic into an error message.
///
/// Guards the per-file `local::import_bytes` + `apply_measured_metadata` step, which runs
/// geometry code on malformed-but-parseable input. Without it, one bad file would abort
/// the rest of the batch (see [`super::wiring::spawn_import`]). With it, the file becomes
/// one more `failed` entry.
///
/// Public because `gui::editor::native_io::catalogue::write_back_to_catalogue` runs the
/// same parse-and-measure step on Save's thread and needs the same isolation.
pub fn catch_file_panic<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string())
    })
}

/// [`import_path`]'s return value.
///
/// Carries the summary shown in the toast, and the `diagram_entries.id` of every design this
/// call saved. The preview-generation offer needs that list.
pub(super) struct ImportOutcome {
    pub(super) summary: String,
    pub(super) imported_ids: Vec<i64>,
    /// Whether at least one candidate failed to read, parse or save. `spawn_import` takes
    /// the toast kind from this, not from the summary text: "Imported 3 file(s); 12 skipped"
    /// starts with "Imported" and would otherwise read as a success.
    pub(super) had_failures: bool,
    /// Whether at least one imported file replaced an existing catalogue row (filename-only
    /// match). With `had_failures`, it keeps the import popup open on completion.
    pub(super) had_collision: bool,
    /// Whether at least one `.gem`/`.gcs` file came with reader or converter warnings. Listed
    /// in `summary`; keeps the import popup open so the list can be read.
    pub(super) had_notes: bool,
}

/// Saves one parsed candidate into `db` and reports whether it replaced an existing row.
///
/// Split out of [`import_path`] for clippy's `too_many_lines` limit.
///
/// A replacement carries the old row's hand-entered metadata forward (see
/// [`merge_reimport_metadata`]) and invalidates its preview and tilt caches, so a regenerate
/// pass rebuilds from the new geometry. A fresh row takes `derived_from_entry_id` only when
/// that source row still exists; a replacement never derives, since it already is that row.
/// Tags and the ignored and planner marks a design file states are applied afterwards by
/// `local::apply_imported_extras`.
///
/// The entry and detail are written by `Database::save_design` in one transaction, so a
/// failure cannot leave a detail-less entry that the next import would read as "no
/// collision". A failed collision check is an error for this file: treating it as "no
/// collision" would skip the merge and the cache invalidation.
///
/// # Errors
///
/// Returns the underlying `Database` error if the collision check or the entry/detail
/// write fails.
fn save_imported_design(
    db: &Arc<Mutex<Database>>,
    url: &str,
    seen_before_in_batch: bool,
    parsed: local::ImportedAsc,
    file_name: &str,
) -> anyhow::Result<(i64, bool)> {
    let local::ImportedAsc {
        entry,
        mut detail,
        derived_from_entry_id,
        extras,
    } = parsed;
    let (id, is_collision) = {
        let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let is_collision = seen_before_in_batch || db.has_detail_for_entry_url(url)?;
        // The replaced row is read BEFORE the write, while its old detail still exists; the
        // url-keyed upsert in `save_design` then reuses this same row.
        if is_collision
            && let Some(existing_id) = db.diagram_entry_id_for_url(&entry.url)?
            && let Ok(Some(existing)) = db.get_diagram_full(existing_id)
        {
            merge_reimport_metadata(&mut detail, &existing);
        }
        let id = db.save_design(&entry, &detail, local::LOCAL_SOURCE_ID)?;
        // Tags, the ignored mark and the planner exclusion a design file carries. Only
        // sets: a re-import never clears what the existing row already has.
        if let Err(e) = local::apply_imported_extras(&db, id, &extras) {
            warn!("Import: could not restore the tags/marks of '{file_name}' on entry #{id}: {e}");
        }
        if is_collision {
            invalidate_stale_caches(&db, id, file_name);
        } else if let Some(source_id) = derived_from_entry_id {
            record_provenance(&db, id, source_id, file_name);
        }
        drop(db);
        (id, is_collision)
    };
    Ok((id, is_collision))
}

/// Deletes `id`'s cached previews, tilt curves and solid extents after a re-import.
///
/// A regenerate pass then rebuilds them from the new geometry. Failures are logged, not
/// propagated, since the import itself succeeded.
fn invalidate_stale_caches(db: &Database, id: i64, file_name: &str) {
    if let Err(e) = db.delete_preview_images(id) {
        warn!(
            "Import: failed to invalidate stale preview cache for entry #{id} \
             ('{file_name}'): {e}"
        );
    }
    if let Err(e) = db.delete_tilt_curves(id) {
        warn!(
            "Import: failed to invalidate stale tilt-curve cache for entry #{id} \
             ('{file_name}'): {e}"
        );
    }
    if let Err(e) = db.delete_solid_extents(id) {
        warn!(
            "Import: failed to invalidate stale solid-extents cache for entry #{id} \
             ('{file_name}'): {e}"
        );
    }
}

/// Records `id` as derived from `source_id`, the catalogue row named by a footnote in the
/// imported file, if that row still exists.
///
/// The footnote is written by `gui::editor::native_io` into this exact file, so it is
/// authoritative; titles and file names are never matched. A failure is logged, not
/// propagated: the import itself succeeded.
fn record_provenance(db: &Database, id: i64, source_id: i64, file_name: &str) {
    match db.get_diagram_full(source_id) {
        Ok(Some(_)) => {
            if let Err(e) = db.set_derived_from_entry_id(id, Some(source_id)) {
                warn!(
                    "Import: failed to record entry #{id} ('{file_name}') as \
                     derived from #{source_id}: {e}"
                );
            }
        }
        Ok(None) => {
            // The recorded source row is gone -- leave provenance unset rather than
            // pointing at a row that no longer exists.
        }
        Err(e) => warn!(
            "Import: could not verify source entry #{source_id} for '{file_name}' \
             before recording provenance: {e}"
        ),
    }
}

/// How many of one file's reader/converter warnings the import summary quotes
/// before it says "+N more" (the log always gets every one).
const WARNINGS_PER_FILE: usize = 3;

/// One summary line for a converted file's warnings: the file name, the first
/// [`WARNINGS_PER_FILE`] warnings, and how many more there were.
fn warning_line(file_name: &str, warnings: &[String]) -> String {
    let shown = warnings
        .iter()
        .take(WARNINGS_PER_FILE)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let rest = warnings.len().saturating_sub(WARNINGS_PER_FILE);
    if rest > 0 {
        format!("{file_name}: {shown} (+{rest} more)")
    } else {
        format!("{file_name}: {shown}")
    }
}

/// Reads and parses one candidate with [`local::import_bytes`].
///
/// A `.gem`/`.gcs` is converted to `.asc` cutting instructions and keeps the original file
/// beside it, a stand-alone `.psdd` imports as it is, and anything else is `.asc` text. A
/// `<stem>.psdd` beside a `.asc`/`.gem`/`.gcs` is attached to it and is what the editor opens.
/// Split out of [`import_path`] for clippy's `too_many_lines` limit.
///
/// The raw bytes go to the vault, which decodes them for parsing (Windows-1252 and
/// byte-order marks included) and stores the file exactly as received. The `.psdd` carries
/// what the bare `.asc` cannot (meet constraints, preform, detached facets, material and RI
/// override; see `pleochroma_formats::native`), so importing only the `.asc` would discard
/// it. [`find_design_file_beside`] looks beside the file itself, whatever was collected;
/// `gui::editor::loading::design_from_full_record` prefers the design file when opening.
///
/// Runs outside the database lock and inside [`catch_file_panic`], so one malformed file
/// cannot take down a folder import.
///
/// # Errors
///
/// A ready-to-list failure line (read, parse or internal error) naming the file.
fn read_and_parse_candidate(
    file_path: &Path,
    file_name: &str,
) -> Result<(local::ImportedAsc, Vec<String>), String> {
    let bytes = std::fs::read(file_path).map_err(|e| format!("{file_name} (read error: {e})"))?;
    let kind = local::ImportKind::from_file_name(file_name);
    let design_file = if kind == local::ImportKind::Design {
        None
    } else {
        find_design_file_beside(file_path)
    };
    let imported = catch_file_panic(std::panic::AssertUnwindSafe(|| {
        local::import_bytes(
            file_name,
            kind,
            &bytes,
            design_file
                .as_ref()
                .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
            apply_measured_metadata,
            |_| {},
        )
    }));
    match imported {
        Ok(Ok(imported)) => Ok((imported.parsed, imported.warnings)),
        Ok(Err(failure)) => Err(format!("{file_name} (parse error: {})", failure.message)),
        Err(panic_msg) => {
            warn!("Import panicked while processing '{file_name}': {panic_msg}");
            Err(format!("{file_name} (internal error: {panic_msg})"))
        }
    }
}

/// Imports every candidate at `path`.
///
/// Candidates are found by [`collect_import_candidates`]. `on_progress(done, total)` is called
/// after each file. Always runs off the UI thread (see `super::wiring::spawn_import`).
///
/// `db` is locked only around each file's database work (the collision check and the
/// writes), never around file I/O, parsing, `.gem`/`.gcs` conversion or measuring. A
/// UI-thread callback that needs `db` therefore waits for one row's write at most.
pub(super) fn import_path(
    db: &Arc<Mutex<Database>>,
    path: &Path,
    recurse: bool,
    mut on_progress: impl FnMut(usize, usize),
) -> ImportOutcome {
    let candidates = match collect_import_candidates(path, recurse) {
        Ok(c) => c,
        Err(message) => {
            return ImportOutcome {
                summary: message,
                imported_ids: Vec::new(),
                had_failures: true,
                had_collision: false,
                had_notes: false,
            };
        }
    };

    let total = candidates.len();
    let mut imported_ids: Vec<i64> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    // Known limitation: the vault dedupes imports on the bare file name (`local://<file_name>`),
    // not the source path, and `diagram_entries.url` is UNIQUE. `round.asc` from two folders
    // therefore replaces the first. The key lives in `pleochroma_vault`; this loop only
    // detects the collision and reports it in the summary.
    let mut seen_in_batch: HashSet<String> = HashSet::new();
    let mut replaced: Vec<String> = Vec::new();
    let mut noted: Vec<String> = Vec::new();

    for (i, file_path) in candidates.into_iter().enumerate() {
        let file_name = file_path.file_name().map_or_else(
            || "unknown.asc".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        on_progress(i + 1, total);

        let url = format!("local://{file_name}");
        // Recorded before parsing, unconditionally, so a batch-internal name
        // collision is still caught even if this occurrence fails to parse or panics.
        let seen_before_in_batch = !seen_in_batch.insert(file_name.clone());

        let (parsed, warnings) = match read_and_parse_candidate(&file_path, &file_name) {
            Ok(result) => result,
            Err(message) => {
                failed.push(message);
                continue;
            }
        };

        match save_imported_design(db, &url, seen_before_in_batch, parsed, &file_name) {
            Ok((id, is_collision)) => {
                imported_ids.push(id);
                if !warnings.is_empty() {
                    tracing::info!("Import: '{file_name}' converted with notes: {warnings:?}");
                    noted.push(warning_line(&file_name, &warnings));
                }
                if is_collision {
                    replaced.push(file_name);
                }
            }
            Err(e) => failed.push(format!("{file_name} (save error: {e})")),
        }
    }

    ImportOutcome {
        summary: import_summary(path, imported_ids.len(), &failed, &replaced, &noted),
        imported_ids,
        had_failures: !failed.is_empty(),
        had_collision: !replaced.is_empty(),
        had_notes: !noted.is_empty(),
    }
}

/// Builds [`import_path`]'s summary text.
///
/// It gives the counts, the skipped files and why, the replaced files, and the converter
/// notes. Each list is capped by [`name_preview`], and the full lists go to the log.
fn import_summary(
    path: &Path,
    imported: usize,
    failed: &[String],
    replaced: &[String],
    noted: &[String],
) -> String {
    use std::fmt::Write as _;
    let mut summary = if failed.is_empty() {
        format!("Imported {imported} file(s).")
    } else {
        warn!(
            "Import from {}: {} failed: {:?}",
            path.display(),
            failed.len(),
            failed
        );
        format!(
            "Imported {imported} file(s); {} skipped ({}).",
            failed.len(),
            name_preview(failed, "; ")
        )
    };
    if !replaced.is_empty() {
        // The full list goes to the log; the popup gets a capped preview.
        tracing::info!(
            "Import from {}: {} replaced by file name: {:?}",
            path.display(),
            replaced.len(),
            replaced
        );
        // The import dialog's `result_text` is the only place this is shown.
        let _ = write!(
            summary,
            " {} design(s) replaced an existing entry with the same file name ({}) -- \
             filename-only matching, not a content comparison.",
            replaced.len(),
            name_preview(replaced, ", ")
        );
    }
    if !noted.is_empty() {
        let _ = write!(
            summary,
            " Converted with notes: {}.",
            name_preview(noted, " | ")
        );
    }
    summary
}

/// How many file names the import summary lists before it says "and N more".
///
/// The popup that shows the summary is a fixed-size dialog; the complete list
/// always goes to the log (`warn!`/`info!` next to the two call sites).
const SUMMARY_NAME_PREVIEW: usize = 8;

/// Joins at most [`SUMMARY_NAME_PREVIEW`] of `names` with `separator`.
///
/// The rest are counted as "and N more", so a large batch cannot turn the summary into a wall
/// of file names.
fn name_preview(names: &[String], separator: &str) -> String {
    let shown = names
        .iter()
        .take(SUMMARY_NAME_PREVIEW)
        .cloned()
        .collect::<Vec<_>>();
    let mut text = shown.join(separator);
    let rest = names.len().saturating_sub(SUMMARY_NAME_PREVIEW);
    if rest > 0 {
        use std::fmt::Write as _;
        let _ = write!(text, "{separator}and {rest} more");
    }
    text
}

#[cfg(test)]
mod summary_tests {
    use super::{SUMMARY_NAME_PREVIEW, name_preview};

    #[test]
    fn a_short_list_is_joined_in_full() {
        let names = vec!["a.asc".to_string(), "b.asc".to_string()];
        assert_eq!(name_preview(&names, ", "), "a.asc, b.asc");
    }

    #[test]
    fn a_long_list_is_capped_with_a_count_of_the_rest() {
        let names: Vec<String> = (0..300).map(|i| format!("f{i}.asc")).collect();
        let text = name_preview(&names, ", ");
        assert!(text.starts_with("f0.asc, f1.asc"));
        assert!(text.ends_with(&format!("and {} more", 300 - SUMMARY_NAME_PREVIEW)));
        assert_eq!(text.matches(".asc").count(), SUMMARY_NAME_PREVIEW);
    }
}
