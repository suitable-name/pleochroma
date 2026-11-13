//! The local search path.
//!
//! It has a request-sequence counter that drops stale results, a debounced background dispatch
//! for every local list query, and [`refresh_diagram_list`], the entry point every local
//! search or filter change calls.

use super::{
    fetch::{
        DisplaySearchOptions, apply_diagram_list_to_ui, fetch_diagram_list_with_own_connection,
    },
    filters::{
        read_id_filter, read_local_only, read_range_filter, read_sort_order, read_tag_filter,
    },
};
use crate::{LibraryModel, MainWindow};
use pleochroma_vault::{
    db::sqlite::{Database, SortOrder},
    model::filter::RangeFilter,
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tracing::error;

/// Debounce between a search or filter change and its background query.
///
/// A burst of range-slider ticks runs one query, for the last position.
const SEARCH_DEBOUNCE_DELAY: Duration = Duration::from_millis(120);

/// How long the search box waits after the last keystroke before the query runs (see
/// [`debounce_text_refresh`]), so typing runs one query rather than one per character.
const TEXT_FILTER_DEBOUNCE: Duration = Duration::from_millis(120);

thread_local! {
    /// Bumped by every [`refresh_diagram_list`] call.
    ///
    /// A dispatch captures the value when requested, and applies its result only if it still
    /// matches on completion (see [`is_current_search`]). A newer search supersedes older ones.
    static SEARCH_SEQ: Cell<u64> = const { Cell::new(0) };
    /// The debounce timer for [`dispatch_background_search`]. Replacing it cancels the
    /// pending one.
    static SEARCH_DEBOUNCE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    /// The timer [`debounce_text_refresh`] restarts on every keystroke. Any refresh that
    /// runs sooner takes it, since that refresh already reads the latest text.
    static TEXT_DEBOUNCE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Runs `refresh` once, [`TEXT_FILTER_DEBOUNCE`] after the last call.
///
/// A call made before that replaces the pending `refresh`. The search box goes through this,
/// so typing does not run a query per character.
///
/// UI thread only (it starts a `slint::Timer`).
pub(super) fn debounce_text_refresh(refresh: impl FnOnce() + 'static) {
    // `slint::Timer` wants an `FnMut`; the `Option` lets the one-shot `refresh` move out.
    let mut refresh = Some(refresh);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        TEXT_FILTER_DEBOUNCE,
        move || {
            if let Some(refresh) = refresh.take() {
                refresh();
            }
        },
    );
    // Replacing the previous timer drops it, which cancels whatever it had pending.
    TEXT_DEBOUNCE.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// Invalidates any search in flight. Also called from outside this module, by
/// [`crate::gui::library::remote`] and `refresh_after_library_change`.
pub(in crate::gui) fn bump_search_seq() -> u64 {
    SEARCH_SEQ.with(|cell| {
        let next = cell.get() + 1;
        cell.set(next);
        next
    })
}

/// Drops the search and text debounce timers a previous load left pending.
///
/// Called by `diagram_list::load_filter_options_and_initial_list`. A leftover timer could bump
/// [`SEARCH_SEQ`] after the new load captured its value, and its closure would keep the old
/// database connection open.
pub(in crate::gui) fn cancel_pending_searches() {
    SEARCH_DEBOUNCE.with(|cell| {
        cell.borrow_mut().take();
    });
    TEXT_DEBOUNCE.with(|cell| {
        cell.borrow_mut().take();
    });
}

/// Whether `seq` is still the latest search.
pub(in crate::gui) fn is_current_search(seq: u64) -> bool {
    SEARCH_SEQ.with(|cell| cell.get() == seq)
}

/// Owned counterpart of [`DisplaySearchOptions`], for the background thread.
///
/// It is `Clone` because the timer closure is `FnMut`.
#[derive(Clone)]
struct BackgroundSearchOptions {
    order: SortOrder,
    local_only: bool,
    tag_filter: Option<String>,
    id_filter: Option<Vec<i64>>,
}

/// Everything [`dispatch_background_search`] needs beyond `ui` and `db_mutex`, bundled to
/// keep that function's argument count within clippy's limit.
#[derive(Clone)]
struct BackgroundSearchRequest {
    /// Captured by [`refresh_diagram_list`] at dispatch time, before the debounce delay.
    seq: u64,
    /// Whether a performance filter makes this the slow query. Only then does the status
    /// line say "Searching...", since a flashing message per slider tick would be noise.
    slow: bool,
    search: String,
    shape_filter: String,
    gear_filter: String,
    range: RangeFilter,
    options: BackgroundSearchOptions,
}

/// Runs a local list query off the UI thread.
///
/// The UI never waits on SQL or the shared database lock. The request is debounced: a timer
/// starts the worker thread, which posts its result back with `Weak::upgrade_in_event_loop`.
/// The result is dropped unless [`is_current_search`] still holds.
///
/// For a slow request, `LibraryModel.status_message` shows "Searching..." at once and is
/// cleared when a current result lands. A fast request leaves the message alone, but clears
/// a stale "Searching..." left by an older slow request.
fn dispatch_background_search(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    request: BackgroundSearchRequest,
) {
    if request.slow {
        ui.global::<LibraryModel>()
            .set_status_message("Searching...".into());
    }

    let ui_weak = ui.as_weak();
    let db_mutex = Arc::clone(db_mutex);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        SEARCH_DEBOUNCE_DELAY,
        move || {
            let db_mutex = Arc::clone(&db_mutex);
            let ui_weak = ui_weak.clone();
            let request = request.clone();
            thread::spawn(move || {
                let BackgroundSearchRequest {
                    seq,
                    slow,
                    search,
                    shape_filter,
                    gear_filter,
                    range,
                    options,
                } = request;
                // Uses its own read-only connection, not the shared `db_mutex`.
                let result = fetch_diagram_list_with_own_connection(
                    &db_mutex,
                    &search,
                    &shape_filter,
                    &gear_filter,
                    &range,
                    DisplaySearchOptions {
                        order: options.order,
                        local_only: options.local_only,
                        tag_filter: options.tag_filter.as_deref(),
                        id_filter: options.id_filter.as_deref(),
                    },
                );
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    if !is_current_search(seq) {
                        // A newer search already owns the display.
                        return;
                    }
                    match result {
                        Ok(fetched) => {
                            apply_diagram_list_to_ui(&ui, fetched);
                            // A fast result must not clobber a different message
                            // (e.g. the "invalid tilt radius" rejection notice).
                            if slow
                                || ui.global::<LibraryModel>().get_status_message()
                                    == "Searching..."
                            {
                                ui.global::<LibraryModel>()
                                    .set_status_message(String::new().into());
                            }
                        }
                        Err(e) => {
                            error!("Failed to search diagrams: {:?}", e);
                            ui.global::<LibraryModel>().set_status_message(
                                format!("Error searching database: {e}").into(),
                            );
                        }
                    }
                });
            });
        },
    );
    SEARCH_DEBOUNCE.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// The local-database search entry point.
///
/// Every local search/filter change in this crate calls this (see
/// `super::refresh_diagram_list_via_source` for the dispatch that picks this over the
/// remote path). The query is debounced and runs on a worker with its own read-only
/// connection; the list updates when it lands -- see [`dispatch_background_search`]'s own
/// doc comment.
pub fn refresh_diagram_list(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    // Drops the debounce timer a previous call left pending. Its result would be dropped by
    // the `seq` bump below anyway, but the query would still run for nothing.
    SEARCH_DEBOUNCE.with(|cell| {
        cell.borrow_mut().take();
    });
    // A keystroke still waiting out its debounce is superseded: this call already runs
    // with the latest text. (When the debounce itself fires into here, this drops the
    // timer that is running -- Slint allows a timer callback to drop its own timer.)
    TEXT_DEBOUNCE.with(|cell| {
        cell.borrow_mut().take();
    });

    // Bumped unconditionally, sync or async -- see `SEARCH_SEQ`'s own doc comment.
    let seq = bump_search_seq();
    let range = read_range_filter(ui);
    let order = read_sort_order(ui);
    let local_only = read_local_only(ui);
    let tag_filter = read_tag_filter(ui);
    let id_filter = read_id_filter(ui);

    // Every query goes through the debounced worker, performance filter or not, so the UI
    // thread never waits on the database.
    let slow = !range.performance.is_empty();
    dispatch_background_search(
        ui,
        db_mutex,
        BackgroundSearchRequest {
            seq,
            slow,
            search: search.to_string(),
            shape_filter: shape_filter.to_string(),
            gear_filter: gear_filter.to_string(),
            range,
            options: BackgroundSearchOptions {
                order,
                local_only,
                tag_filter,
                id_filter,
            },
        },
    );
}
