//! The "regenerate for the filtered set" library actions: resolving the panel's
//! filtered set off the UI thread and handing it to the batch confirm step.

use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{
        library::{
            read_access::with_read_db,
            search::{FilterSnapshot, resolve_filtered_entry_ids},
        },
        show_toast,
    },
};
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Wires `LibraryModel::regenerate_previews_for_filtered_set`/
/// `regenerate_tilt_curves_for_filtered_set` -- the owner's "regenerate library
/// previews/tilt curves for designs already in the catalogue" request,
/// for the panel's currently active search/filter set rather than a
/// single design or the whole catalogue.
///
/// Each callback resolves the filtered set on a worker
/// ([`resolve_filtered_set_async`]) and hands the result straight to the existing per-batch confirm-step opener
/// (`gui::batch::preview::offer_batch_confirmation`/`gui::batch::tilt::
/// offer_batch_confirmation`) -- the same dialog, progress, and cancel machinery
/// every other trigger of these two batches already uses; this adds no second copy
/// of either batch's generation logic. Refuses (with a toast) under a
/// [`LibrarySource::Remote`] session, since [`Database::matching_entry_ids`] only
/// ever sees the local catalogue -- silently running it there would offer to
/// regenerate an unrelated, likely empty or wrong, local id set for whatever the
/// panel is showing from the remote worker.
///
/// Also a no-op (with a toast, not a silent nothing) when the filtered set is
/// empty, since `offer_batch_confirmation`
/// itself only skips silently -- a cutter who just pressed the button deserves to
/// know why nothing opened.
pub(in crate::gui) fn setup_regenerate_filtered_set_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_previews = Arc::clone(db);
    let source_previews = Arc::clone(source);
    let ui_weak_previews = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_regenerate_previews_for_filtered_set(move || {
            let Some(ui) = ui_weak_previews.upgrade() else {
                return;
            };
            resolve_filtered_set_async(
                &ui,
                &db_previews,
                &source_previews,
                |ui: &MainWindow, ids: Vec<i64>| {
                    crate::gui::batch::regenerate_all::offer_preview_regeneration(ui, &ids);
                },
            );
        });

    let db_tilt = Arc::clone(db);
    let source_tilt = Arc::clone(source);
    let ui_weak_tilt = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_regenerate_tilt_curves_for_filtered_set(move || {
            let Some(ui) = ui_weak_tilt.upgrade() else {
                return;
            };
            resolve_filtered_set_async(
                &ui,
                &db_tilt,
                &source_tilt,
                |ui: &MainWindow, ids: Vec<i64>| {
                    crate::gui::batch::regenerate_all::offer_tilt_regeneration(ui, &ids);
                },
            );
        });
}

/// Set from the click that starts a filtered-set resolution until its completion
/// callback (or a failed spawn) clears it, so a second click while the uncapped query is
/// still running is refused instead of queueing a second scan and a second dialog. Only
/// the UI thread reads it before setting it, so a plain load/store pair is race-free.
static FILTERED_SET_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// What [`admit`] decides for one click on a "regenerate for the filtered set" action.
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    /// Start resolving.
    Admit,
    /// A previous click's resolution has not finished.
    StillResolving,
    /// The library is a remote worker's; the local filtered set does not apply.
    RemoteSource,
}

/// The pure admission decision for [`resolve_filtered_set_async`]. The remote refusal
/// wins: its message tells the cutter what to change, the in-flight one only to wait.
const fn admit(in_flight: bool, is_remote: bool) -> Admission {
    if is_remote {
        Admission::RemoteSource
    } else if in_flight {
        Admission::StillResolving
    } else {
        Admission::Admit
    }
}

/// Shared body of both `setup_regenerate_filtered_set_callbacks` handlers: the
/// remote-source guard, the filtered-set query (on a worker, over a read-only
/// connection, never under the UI thread), and the "nothing matched"/query-failure
/// toasts. `on_ready` runs on the UI thread with the resolved, non-empty id list.
///
/// The panel's filter state is captured as a [`FilterSnapshot`] before the worker
/// starts. When the worker finishes, the source must still be local and a fresh capture
/// must equal the snapshot; otherwise the ids describe a view the cutter has left, and
/// the cutter is told to run the action again instead of being shown a dialog for the
/// wrong set. (There is no shared "current search sequence" getter to compare as well:
/// the snapshot equality covers every input a search sequence bump follows from.)
/// At most one resolution runs at a time. The result is uncapped, exactly as
/// `current_filtered_entry_ids` returned it.
pub(in crate::gui) fn resolve_filtered_set_async(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    on_ready: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    let is_remote = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_remote();
    match admit(FILTERED_SET_IN_FLIGHT.load(Ordering::Acquire), is_remote) {
        Admission::RemoteSource => {
            show_toast(
                ui,
                "Switch to the local library to regenerate designs for the filtered set.",
                "error",
            );
            return;
        }
        Admission::StillResolving => {
            show_toast(ui, "Still resolving the filtered set.", "info");
            return;
        }
        Admission::Admit => {}
    }
    let snapshot = FilterSnapshot::capture(ui);
    FILTERED_SET_IN_FLIGHT.store(true, Ordering::Release);
    ui.global::<LibraryModel>()
        .set_status_message("Resolving the filtered set…".into());

    let ui_weak = ui.as_weak();
    let db = Arc::clone(db);
    let source = Arc::clone(source);
    let spawned = std::thread::Builder::new()
        .name("library-filtered-set".into())
        .spawn(move || {
            let resolved = with_read_db(&db, |db| resolve_filtered_entry_ids(db, &snapshot));
            let queued = ui_weak.upgrade_in_event_loop(move |ui| {
                FILTERED_SET_IN_FLIGHT.store(false, Ordering::Release);
                let still_local = !source
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_remote();
                if !still_local || FilterSnapshot::capture(&ui) != snapshot {
                    show_toast(&ui, "The filters changed; run the action again.", "info");
                    return;
                }
                match resolved {
                    Ok(ids) if ids.is_empty() => {
                        show_toast(&ui, "No designs match the current filters.", "info");
                    }
                    Ok(ids) => on_ready(&ui, ids),
                    Err(e) => show_toast(
                        &ui,
                        &format!("Could not resolve the filtered set: {e}"),
                        "error",
                    ),
                }
            });
            if queued.is_err() {
                // The window is gone; nothing will run the callback above.
                FILTERED_SET_IN_FLIGHT.store(false, Ordering::Release);
            }
        });
    if let Err(e) = spawned {
        FILTERED_SET_IN_FLIGHT.store(false, Ordering::Release);
        show_toast(
            ui,
            &format!("Could not start resolving the filtered set: {e}"),
            "error",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Admission, admit};

    #[test]
    fn a_second_click_while_resolving_is_refused() {
        assert_eq!(admit(true, false), Admission::StillResolving);
        assert_eq!(admit(false, false), Admission::Admit);
    }

    #[test]
    fn a_remote_source_is_refused_before_the_in_flight_check() {
        assert_eq!(admit(false, true), Admission::RemoteSource);
        assert_eq!(admit(true, true), Admission::RemoteSource);
    }
}
