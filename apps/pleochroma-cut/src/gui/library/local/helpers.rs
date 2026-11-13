//! Shared helpers for local library writes. [`import`], [`organize`] and [`export`] all end
//! with [`refresh_after_library_change`].
//!
//! [`import`]: super::import
//! [`organize`]: super::organize
//! [`export`]: super::export

use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::library::{
        diagram_list::apply_attribute_range_bounds_preserving_filters,
        search::{
            DisplaySearchOptions, apply_diagram_list_to_ui, bump_search_seq,
            fetch_list_and_ranges_with_own_connection, is_current_search, read_id_filter,
            read_local_only, read_range_filter, read_sort_order, read_tag_filter,
        },
    },
};
use pleochroma_vault::db::sqlite::{Database, SortOrder};
use slint::{ComponentHandle, Model, Weak};
use std::{
    sync::{Arc, Mutex},
    thread,
};
use tracing::warn;

/// Re-runs the catalogue query with the search and filters currently on screen. This is the
/// shared refresh after an import, rename, delete or shape change.
///
/// Those writes touch only the local database, so the attribute-range refresh runs only while
/// the local library is active. A remote view refreshes through
/// [`crate::gui::library::search::refresh_diagram_list_via_source`] and keeps showing remote
/// results.
///
/// The local path runs its two reads on a worker over a read-only connection, then applies the
/// results on the UI thread. The reads follow a write, so the extra thread hop is not
/// noticeable. A synchronous read on a large catalogue would freeze the window.
pub fn refresh_after_library_change(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => {
            // A write that can create/empty a tag (add/remove
            // tag) needs the chip filter row's own vocabulary refreshed alongside
            // the list -- cheap enough (one query) to run synchronously here rather
            // than threading it through `spawn_local_refresh`'s worker thread.
            crate::gui::library::diagram_list::sync_tag_vocabulary_to_ui(ui, db);
            let search = ui.global::<LibraryModel>().get_search_text().to_string();
            let shape_idx = usize::try_from(ui.global::<LibraryModel>().get_selected_shape_index())
                .unwrap_or(usize::MAX);
            let shape = ui
                .global::<LibraryModel>()
                .get_shape_options()
                .row_data(shape_idx)
                .unwrap_or_default()
                .to_string();
            let gear_idx = usize::try_from(ui.global::<LibraryModel>().get_selected_gear_index())
                .unwrap_or(usize::MAX);
            let gear = ui
                .global::<LibraryModel>()
                .get_gear_options()
                .row_data(gear_idx)
                .unwrap_or_default()
                .to_string();
            // This refresh follows a write, so the cutter's range filters, sort and
            // "My designs" choice must survive it. They are read here, since the worker
            // has no `MainWindow`.
            //
            // The sequence is bumped before the fetch starts, and `spawn_local_refresh`
            // checks it before it touches the UI.
            let seq = bump_search_seq();
            let query = DiagramListQuery {
                seq,
                search,
                shape,
                gear,
                range: read_range_filter(ui),
                order: read_sort_order(ui),
                local_only: read_local_only(ui),
                tag_filter: read_tag_filter(ui),
                id_filter: read_id_filter(ui),
            };
            spawn_local_refresh(ui.as_weak(), Arc::clone(db), query);
        }
        LibrarySource::Remote(_) => {
            let search = ui.global::<LibraryModel>().get_search_text();
            let shape_idx = usize::try_from(ui.global::<LibraryModel>().get_selected_shape_index())
                .unwrap_or(usize::MAX);
            let shape = ui
                .global::<LibraryModel>()
                .get_shape_options()
                .row_data(shape_idx)
                .unwrap_or_default();
            let gear_idx = usize::try_from(ui.global::<LibraryModel>().get_selected_gear_index())
                .unwrap_or(usize::MAX);
            let gear = ui
                .global::<LibraryModel>()
                .get_gear_options()
                .row_data(gear_idx)
                .unwrap_or_default();
            crate::gui::library::search::refresh_diagram_list_via_source(
                ui, db, source, &search, &shape, &gear,
            );
        }
    }
}

/// The catalogue list's search, filters and sort order as they were on screen when the write
/// finished. The UI thread snapshots them, since the worker cannot read the window.
struct DiagramListQuery {
    /// The search sequence number, taken at dispatch time. It is checked with
    /// `is_current_search`, as in the background search.
    seq: u64,
    search: String,
    shape: String,
    gear: String,
    range: pleochroma_vault::model::filter::RangeFilter,
    order: SortOrder,
    local_only: bool,
    tag_filter: Option<String>,
    id_filter: Option<Vec<i64>>,
}

/// Runs the attribute-range read and the list query on a worker thread.
///
/// Both results are applied on the UI thread in one hop. A keystroke or filter change made while
/// this is in flight bumps the sequence. The [`crate::gui::library::search::is_current_search`]
/// check stops these older results from overwriting that newer list.
fn spawn_local_refresh(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    query: DiagramListQuery,
) {
    thread::spawn(move || {
        let seq = query.seq;
        // One read-only connection serves both reads, so the shared database lock is not
        // held during the fetch.
        let (ranges, list_result) = fetch_list_and_ranges_with_own_connection(
            &db,
            &query.search,
            &query.shape,
            &query.gear,
            &query.range,
            DisplaySearchOptions {
                order: query.order,
                local_only: query.local_only,
                tag_filter: query.tag_filter.as_deref(),
                id_filter: query.id_filter.as_deref(),
            },
        );
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if !is_current_search(seq) {
                return;
            }
            if let Some(ranges) = ranges {
                // This refresh follows a write, so the cutter's range filters are kept
                // rather than reset.
                apply_attribute_range_bounds_preserving_filters(&ui, &ranges);
            }
            match list_result {
                Ok(fetched) => apply_diagram_list_to_ui(&ui, fetched),
                Err(e) => {
                    warn!("Failed to search diagrams: {e:?}");
                    ui.global::<LibraryModel>()
                        .set_status_message(format!("Error searching database: {e}").into());
                }
            }
        });
    });
}

#[cfg(test)]
pub(super) mod test_support {
    use pleochroma_vault::db::sqlite::Database;
    use std::{
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        },
    };

    pub(in crate::gui::library::local) const VALID_ASC: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H Round Trichecker-12\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n";

    /// A fresh, empty scratch directory under the OS temp dir -- same naming
    /// convention `pleochroma_vault::db::sqlite::tests::temp_db_path` uses for its own
    /// throwaway files, extended to a directory here.
    pub(in crate::gui::library::local) fn temp_dir_for_test(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pleochroma_cut_test_{label}_{n}_{}",
            std::process::id()
        ));
        // Windows reuses process ids, so an earlier (killed) test run can have left a
        // directory with this exact name behind; start from an empty one.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    /// A fresh path for a throwaway SQLite file, never the real `facet_diagrams.sqlite`.
    /// Each test builds its own database with [`Database::new`].
    pub(in crate::gui::library::local) fn temp_db_path_for_test(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pleochroma_cut_test_db_{label}_{n}_{}.sqlite",
            std::process::id()
        ));
        // Windows reuses process ids, so an earlier (killed) test run can have left a
        // database with this exact name behind; a test that builds its own schema on
        // it then fails with "table ... already exists".
        let _ = std::fs::remove_file(&path);
        path
    }

    pub(in crate::gui::library::local) fn open_temp_db(
        path: &std::path::Path,
    ) -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::new(Some(path.to_str().expect("temp path is valid UTF-8")))
                .expect("create fresh temp test db"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use pleochroma_vault::db::sqlite::Database;
    use std::path::Path;

    /// Manual perf probe: times `get_attribute_ranges` and an unfiltered `search_diagrams` (the
    /// post-write refresh reads) on a real `facet_diagrams.sqlite`, opened read-only. Ignored
    /// because that file is not in the repository. Run it with
    /// `cargo test -p pleochroma-cut -- --ignored perf_probe --nocapture`.
    #[test]
    #[ignore = "manual perf probe against the real local catalogue, not for CI"]
    fn perf_probe_refresh_after_library_change_cost() {
        let real_db_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../facet_diagrams.sqlite");
        if !Path::new(real_db_path).is_file() {
            eprintln!("skipping: {real_db_path} not found on this machine");
            return;
        }
        let db = Database::open_read_only(real_db_path).expect("open real catalogue read-only");

        let t0 = std::time::Instant::now();
        let ranges = db.get_attribute_ranges().expect("get_attribute_ranges");
        let ranges_elapsed = t0.elapsed();

        let range = pleochroma_vault::model::filter::RangeFilter::default();
        let t1 = std::time::Instant::now();
        let items = db
            .search_diagrams("", "All", "All", &range)
            .expect("search_diagrams");
        let search_elapsed = t1.elapsed();

        let t2 = std::time::Instant::now();
        let total = db.get_total_count().expect("get_total_count");
        let count_elapsed = t2.elapsed();

        eprintln!(
            "get_attribute_ranges: {ranges_elapsed:?} ({ranges:?})\n\
             search_diagrams (unfiltered): {search_elapsed:?} ({} rows)\n\
             get_total_count: {count_elapsed:?} ({total} designs)",
            items.len(),
        );
        // Not a hard perf assertion (machine-dependent) -- this is a measurement
        // probe, not a regression gate.
    }
}
