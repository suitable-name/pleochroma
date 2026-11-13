//! Wiring for switching the browsed library source (`switch_library_source`) and for the
//! pull-mirror sync (`start_mirror_sync`/`cancel_mirror_sync`).
//!
//! The decisions about local or remote and about mirroring live in `bridge::library`. This
//! module is the Slint-callback glue: blocking calls run on a `std::thread::spawn` worker, and
//! results return through `Weak::upgrade_in_event_loop`. A running sync's handle is kept on the
//! UI thread so a later "Cancel" click can reach it.

use crate::{
    LibraryModel, MainWindow, RemoteWorkerModel,
    bridge::library::{
        client as library_client,
        mirror::{self as library_mirror, MirrorHandle, MirrorOutcome, MirrorProgress},
        source::{LibrarySource, spawn_library_request},
    },
    gui::show_toast,
    settings::{SettingsPersister, WorkerSettings},
};
use pleochroma_net::library::{AttributeRangesWire, LibraryRequest, LibraryResponse};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

/// Wires the "Local"/"Browse library" switch (see `remote_worker_dialog.slint`).
///
/// Switching to a worker first probes it on a background thread (handshake only). The
/// source becomes [`LibrarySource::Remote`], and the badge and the worker's filters and list
/// load, only when the probe succeeds and the worker advertises library capacity
/// (`ConnectionInfo::library`). Otherwise a toast explains why and the viewer stays on its
/// current library, never in a half-switched state.
pub fn setup_library_source_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let db_switch = Arc::clone(db);
    let source_switch = Arc::clone(source);
    let settings_switch = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_switch_library_source(move |idx: i32| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };

        if idx < 0 {
            // Drops any remote search reply still in flight. The local reload below bumps the
            // sequence only once its own query finishes, so this covers the gap.
            crate::gui::library::search::bump_search_seq();
            // A detail load of the other library still in flight must not land here.
            crate::gui::library::detail::invalidate_detail_loads();
            *source_switch
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = LibrarySource::Local;
            apply_source_badge(&ui, &LibrarySource::Local);
            ui.global::<RemoteWorkerModel>().set_library_source_worker_index(-1);
            crate::gui::library::diagram_list::load_filter_options_and_initial_list(&ui, &db_switch);
            show_toast(&ui, "Switched to the local library.", "info");
            return;
        }

        // Any non-negative index is the one remote endpoint (only one can be configured).
        let Some(worker) = settings_switch.snapshot().settings.remote_worker() else {
            show_toast(&ui, "No remote coordinator is configured.", "error");
            return;
        };

        ui.global::<LibraryModel>().set_status_message(format!("Connecting to {}...", worker_display_name(&worker)).into());
        // The sequence is bumped before the probe starts. A second switch clicked meanwhile
        // bumps it again, so this reply can tell it has been superseded and must not commit
        // `LibrarySource::Remote`.
        let switch_seq = crate::gui::library::search::bump_search_seq();
        crate::gui::library::detail::invalidate_detail_loads();
        let source_probe = Arc::clone(&source_switch);
        let ui_weak_result = ui.as_weak();
        let worker_for_probe = worker;
        std::thread::spawn(move || {
            let result = library_client::probe(&worker_for_probe);
            let _ = ui_weak_result.upgrade_in_event_loop(move |ui| {
                if !crate::gui::library::search::is_current_search(switch_seq) {
                    // A newer switch already owns the display, so this reply must not
                    // touch `source` or the UI, whether it succeeded or not.
                    return;
                }
                match result {
                Ok(info) if info.library => {
                    // Drops any search still in flight from the previous library.
                    crate::gui::library::search::bump_search_seq();
                    let new_source = LibrarySource::Remote(worker_for_probe);
                    *source_probe.lock().unwrap_or_else(PoisonError::into_inner) =
                        new_source.clone();
                    apply_source_badge(&ui, &new_source);
                    ui.global::<RemoteWorkerModel>().set_library_source_worker_index(0);
                    if let Some(w) = new_source.worker() {
                        load_filter_options_and_initial_list_remote(&ui, w.clone());
                    }
                    show_toast(&ui, &format!("Now browsing: {}", new_source.label()), "success");
                    // The sort and tag filters are sent to the remote search, so they work.
                    // "My designs" and the just-imported restriction name local entry ids
                    // and are greyed out for a remote library. Say so once.
                    ui.global::<LibraryModel>().set_status_message(
                        "\"My designs\" and the just-imported filter apply to the local \
                         library only."
                            .into(),
                    );
                }
                Ok(_) => {
                    show_toast(
                        &ui,
                        "That remote is reachable but does not serve a design library -- staying on the current library.",
                        "error",
                    );
                    ui.global::<LibraryModel>().set_status_message("Ready.".into());
                }
                Err(e) => {
                    show_toast(&ui, &format!("Could not switch library: {e}"), "error");
                    ui.global::<LibraryModel>().set_status_message("Ready.".into());
                }
                }
            });
        });
    });
}

/// Sets the header badge's label and remote flag to reflect `source`.
///
/// `library_source_worker_index` is set separately at each call site, since `LibrarySource`
/// holds only the resolved `WorkerSettings`, not a list index.
fn apply_source_badge(ui: &MainWindow, source: &LibrarySource) {
    ui.global::<RemoteWorkerModel>()
        .set_library_source_label(source.label().into());
    ui.global::<RemoteWorkerModel>()
        .set_library_is_remote(source.is_remote());
    crate::gui::rough_plan::refresh_links_enabled();
}

/// Loads `worker`'s filter options (shapes, gears, attribute ranges) and its initial diagram
/// list. This is the remote counterpart of
/// `gui::library::diagram_list::load_filter_options_and_initial_list`.
fn load_filter_options_and_initial_list_remote(ui: &MainWindow, worker: WorkerSettings) {
    // The sequence is captured before the request. A late `FilterOptions` reply must not
    // reset the dropdowns after the cutter has switched library, and must not start a
    // search for a worker no longer shown.
    let seq = crate::gui::library::search::bump_search_seq();
    let worker_for_search = worker.clone();
    crate::bridge::library::source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FilterOptions,
        move |ui, result| {
            if !crate::gui::library::search::is_current_search(seq) {
                return;
            }
            match result {
                Ok(LibraryResponse::FilterOptions {
                    shapes,
                    gears,
                    ranges,
                }) => {
                    apply_shape_gear_options(ui, &shapes, &gears);
                    apply_range_wire_to_ui(ui, &ranges);
                    // The search box, tag chips and just-imported restriction are not part of
                    // this reply, so reset them here. Otherwise they keep the previous
                    // library's values while the query below runs with empty filters.
                    crate::gui::library::diagram_list::reset_search_and_filter_ui_inputs(ui);
                }
                Ok(_) => {
                    ui.global::<LibraryModel>().set_status_message(
                        "Unexpected reply loading remote filter options.".into(),
                    );
                }
                Err(e) => ui.global::<LibraryModel>().set_status_message(
                    format!("Could not load remote filter options: {e}").into(),
                ),
            }
            // Chained, not parallel: each request opens its own connection, and the list
            // needs the filter options already loaded.
            crate::gui::library::search::refresh_diagram_list_remote(
                ui,
                worker_for_search,
                "",
                "All Shapes",
                "All Gears",
            );
        },
    );
}

/// A worker's display name for a status message: its `name` if set, else its `address`.
///
/// This is [`LibrarySource::label`]'s fallback without the `"Remote: "` prefix, which does not
/// read well mid-sentence.
fn worker_display_name(worker: &WorkerSettings) -> &str {
    if worker.name.trim().is_empty() {
        &worker.address
    } else {
        &worker.name
    }
}

fn apply_shape_gear_options(ui: &MainWindow, shapes: &[String], gears: &[String]) {
    let mut shape_opts = vec!["All Shapes".to_string()];
    shape_opts.extend(shapes.iter().cloned());
    let shape_model: Vec<SharedString> = shape_opts.into_iter().map(Into::into).collect();
    ui.global::<LibraryModel>()
        .set_shape_options(ModelRc::new(VecModel::from(shape_model)));

    let mut gear_opts = vec!["All Gears".to_string()];
    gear_opts.extend(gears.iter().cloned());
    let gear_model: Vec<SharedString> = gear_opts.into_iter().map(Into::into).collect();
    ui.global::<LibraryModel>()
        .set_gear_options(ModelRc::new(VecModel::from(gear_model)));
    ui.global::<LibraryModel>().set_selected_shape_index(0);
    ui.global::<LibraryModel>().set_selected_gear_index(0);
}

/// A catalogue range bound (stored as `f64`) as the `f32` a Slint slider property takes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Slint slider properties are f32; catalogue bounds are small values in the normal float range"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

fn apply_range_wire_to_ui(ui: &MainWindow, ranges: &AttributeRangesWire) {
    ui.global::<LibraryModel>()
        .set_ri_bounds_min(to_f32(ranges.ri.0));
    ui.global::<LibraryModel>()
        .set_ri_bounds_max(to_f32(ranges.ri.1));
    ui.global::<LibraryModel>()
        .set_ri_filter_min(to_f32(ranges.ri.0));
    ui.global::<LibraryModel>()
        .set_ri_filter_max(to_f32(ranges.ri.1));
    ui.global::<LibraryModel>()
        .set_lw_bounds_min(to_f32(ranges.lw_ratio.0));
    ui.global::<LibraryModel>()
        .set_lw_bounds_max(to_f32(ranges.lw_ratio.1));
    ui.global::<LibraryModel>()
        .set_lw_filter_min(to_f32(ranges.lw_ratio.0));
    ui.global::<LibraryModel>()
        .set_lw_filter_max(to_f32(ranges.lw_ratio.1));
    ui.global::<LibraryModel>()
        .set_volume_bounds_min(to_f32(ranges.volume.0));
    ui.global::<LibraryModel>()
        .set_volume_bounds_max(to_f32(ranges.volume.1));
    ui.global::<LibraryModel>()
        .set_volume_filter_min(to_f32(ranges.volume.0));
    ui.global::<LibraryModel>()
        .set_volume_filter_max(to_f32(ranges.volume.1));
    ui.global::<LibraryModel>()
        .set_facets_bounds_min(ranges.facets.0 as f32);
    ui.global::<LibraryModel>()
        .set_facets_bounds_max(ranges.facets.1 as f32);
    ui.global::<LibraryModel>()
        .set_facets_filter_min(ranges.facets.0 as f32);
    ui.global::<LibraryModel>()
        .set_facets_filter_max(ranges.facets.1 as f32);
}

/// Wires "Mirror to local" / "Cancel" (see `remote_worker_dialog.slint`).
///
/// Only one sync runs at a time. The UI disables "Mirror to local" while `mirror_in_progress`
/// is set. This code does not check the handle before a new sync and simply overwrites it.
pub fn setup_mirror_sync_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let handle: Rc<RefCell<Option<MirrorHandle>>> = Rc::new(RefCell::new(None));

    let db_start = Arc::clone(db);
    let settings_start = settings_store.clone();
    let handle_start = handle.clone();
    let ui_weak_start = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_start_mirror_sync(move || {
            let Some(ui) = ui_weak_start.upgrade() else {
                return;
            };
            let Some(worker) = settings_start.snapshot().settings.remote_worker() else {
                show_toast(&ui, "No remote coordinator is configured.", "error");
                return;
            };

            ui.global::<RemoteWorkerModel>()
                .set_mirror_in_progress(true);
            ui.global::<RemoteWorkerModel>()
                .set_mirror_progress_fraction(0.0);
            ui.global::<RemoteWorkerModel>().set_mirror_has_error(false);
            ui.global::<RemoteWorkerModel>()
                .set_mirror_status_text("Listing the remote library...".into());

            let new_handle = library_mirror::spawn_mirror_sync(
                ui.as_weak(),
                Arc::clone(&db_start),
                worker,
                library_mirror::MirrorOptions::default(),
                |ui, progress| on_mirror_progress(ui, &progress),
                |ui, outcome| on_mirror_done(ui, &outcome),
            );
            *handle_start.borrow_mut() = Some(new_handle);
        });

    let handle_cancel = handle;
    ui.global::<RemoteWorkerModel>()
        .on_cancel_mirror_sync(move || {
            if let Some(h) = handle_cancel.borrow().as_ref() {
                h.cancel();
            }
        });
}

fn on_mirror_progress(ui: &MainWindow, progress: &MirrorProgress) {
    let total = progress.counts.total_found.max(1);
    let fraction = progress.processed as f32 / total as f32;
    ui.global::<RemoteWorkerModel>()
        .set_mirror_progress_fraction(fraction);
    ui.global::<RemoteWorkerModel>().set_mirror_status_text(
        format!(
            "{}/{} -- {} (new {}, updated {}, unchanged {}, local conflicts {}, failed {})",
            progress.processed,
            progress.counts.total_found,
            progress.current_title,
            progress.counts.new_count,
            progress.counts.updated_count,
            progress.counts.skipped_unchanged,
            progress.counts.local_conflicts_skipped,
            progress.counts.failed,
        )
        .into(),
    );
}

/// A trailing `" (N designs skipped -- deleted locally.)"` clause for the mirror summary.
///
/// It is empty when `orphaned` is `0`. A local delete wins, so these designs never return on
/// their own.
fn orphaned_mirror_states_suffix(orphaned: u64) -> String {
    if orphaned == 0 {
        String::new()
    } else {
        format!(" ({orphaned} design(s) skipped -- deleted locally.)")
    }
}

fn on_mirror_done(ui: &MainWindow, outcome: &MirrorOutcome) {
    ui.global::<RemoteWorkerModel>()
        .set_mirror_in_progress(false);
    let (text, is_error, toast_kind) = match outcome {
        MirrorOutcome::Completed(c) => (
            format!(
                "Mirror complete: {} new, {} updated, {} unchanged, {} deleted locally \
                 (skipped), {} local conflicts skipped, {} failed (of {} found).{}",
                c.new_count,
                c.updated_count,
                c.skipped_unchanged,
                c.skipped_deleted,
                c.local_conflicts_skipped,
                c.failed,
                c.total_found,
                orphaned_mirror_states_suffix(c.orphaned_mirror_states),
            ),
            c.failed > 0,
            if c.failed > 0 { "error" } else { "success" },
        ),
        MirrorOutcome::Cancelled(c) => (
            format!(
                "Mirror cancelled after {} new, {} updated, {} unchanged, {} deleted \
                 locally (skipped), {} local conflicts skipped (of {} found so far).{}",
                c.new_count,
                c.updated_count,
                c.skipped_unchanged,
                c.skipped_deleted,
                c.local_conflicts_skipped,
                c.total_found,
                orphaned_mirror_states_suffix(c.orphaned_mirror_states),
            ),
            false,
            "info",
        ),
        MirrorOutcome::Failed(msg) => (format!("Mirror failed: {msg}"), true, "error"),
    };
    ui.global::<RemoteWorkerModel>()
        .set_mirror_has_error(is_error);
    ui.global::<RemoteWorkerModel>()
        .set_mirror_status_text(text.clone().into());
    show_toast(ui, &text, toast_kind);
}

/// One remote design's original `.asc` text, fetched by [`fetch_remote_design_source`].
///
/// It is the text a local `.asc` attachment carries, not a rebuild from the angle table. The
/// editor builds the `Design` from it, as for local designs.
#[derive(Debug)]
pub struct RemoteDesignSource {
    /// The attachment's bare file name (e.g. `"round-brilliant.asc"`), used as
    /// `EditorState::asc_filename`.
    pub file_name: String,
    /// The attachment's exact text. It is parsed into the `Design`, and kept as
    /// `EditorState::original_asc_text` so "Export .asc" can write the same bytes back.
    pub asc_text: String,
    /// What the remote design states as frosted, by tier position and facet slot of the
    /// schedule `asc_text` describes (`.asc` has no notation for it, so it travels beside
    /// the text). Apply it with `Design::apply_imported_frosting` after the design is
    /// built from `asc_text`; empty for a design without frosting.
    pub frosting: Vec<(usize, pleochroma_formats::gcs::GcsTierFrosting)>,
}

/// Fetches `entry_id`'s original `.asc` text from `worker`, off the UI thread.
///
/// `on_done` runs on the Slint event loop. Its `Err` is a ready-to-toast message for a
/// transport failure, a missing entry, an entry with no attached `.asc`, a server error, or an
/// unexpected reply.
///
/// Used by the editor to open a remote design: `gui::editor::callbacks::tier_actions::lifecycle`
/// and `gui::editor::guide::build_design`.
pub fn fetch_remote_design_source(
    ui_weak: Weak<MainWindow>,
    worker: WorkerSettings,
    entry_id: i64,
    on_done: impl FnOnce(&MainWindow, Result<RemoteDesignSource, String>) + Send + 'static,
) {
    spawn_library_request(
        ui_weak,
        worker,
        LibraryRequest::FetchDesignSource { entry_id },
        move |ui, result| on_done(ui, map_design_source_response(result)),
    );
}

/// Maps a reply to the [`fetch_remote_design_source`] outcome.
///
/// It is a separate pure function, so it can be tested without a network round trip.
fn map_design_source_response(
    result: Result<LibraryResponse, library_client::LibraryClientError>,
) -> Result<RemoteDesignSource, String> {
    match result {
        Ok(LibraryResponse::DesignSource {
            file_name,
            asc_text,
            frosting,
            ..
        }) => Ok(RemoteDesignSource {
            file_name,
            asc_text,
            frosting: pleochroma_vault::model::frosting::StoredTierFrosting::to_gcs(
                &crate::bridge::library::frosting_from_wire(&frosting),
            ),
        }),
        Ok(LibraryResponse::NotFound) => {
            Err("That design no longer exists on the remote library.".to_string())
        }
        Ok(LibraryResponse::DesignSourceNotAvailable) => Err(
            "This design has no attached .asc, .gem or .gcs file on the remote library -- \
             nothing to load."
                .to_string(),
        ),
        Ok(LibraryResponse::Error(e)) => Err(e.message),
        Ok(_) => Err("Unexpected reply fetching the remote design source.".to_string()),
        Err(e) => Err(format!("Could not reach the remote library: {e}")),
    }
}

#[cfg(test)]
mod remote_design_source_tests {
    use super::{RemoteDesignSource, map_design_source_response};
    use pleochroma_net::{library::LibraryResponse, messages::ErrorMsg};

    #[test]
    fn a_design_source_reply_maps_to_ok() {
        let result = map_design_source_response(Ok(LibraryResponse::DesignSource {
            entry_id: 7,
            file_name: "round-brilliant.asc".to_string(),
            asc_text: "GemCad 5.0\n".to_string(),
            frosting: Vec::new(),
        }));
        let RemoteDesignSource {
            file_name,
            asc_text,
            frosting,
        } = result.expect("DesignSource must map to Ok");
        assert_eq!(file_name, "round-brilliant.asc");
        assert_eq!(asc_text, "GemCad 5.0\n");
        assert_eq!(
            frosting,
            [] as [(usize, pleochroma_formats::gcs::GcsTierFrosting); 0]
        );
    }

    /// The frosting the worker sends beside the text reaches the editor in the form
    /// `Design::apply_imported_frosting` takes: tier positions and facet slots, ascending.
    #[test]
    fn a_design_source_reply_carries_its_frosting_to_the_editor() {
        use pleochroma_formats::gcs::GcsTierFrosting;
        use pleochroma_net::library::FrostedTierWire;
        let result = map_design_source_response(Ok(LibraryResponse::DesignSource {
            entry_id: 7,
            file_name: "frosted.asc".to_string(),
            asc_text: "GemCad 5.0\n".to_string(),
            frosting: vec![
                FrostedTierWire {
                    tier: 3,
                    facets: vec![1, 3],
                },
                FrostedTierWire {
                    tier: 1,
                    facets: Vec::new(),
                },
            ],
        }));
        let frosting = result.expect("DesignSource must map to Ok").frosting;
        assert_eq!(
            frosting,
            vec![
                (1, GcsTierFrosting::Whole),
                (3, GcsTierFrosting::Facets(vec![1, 3])),
            ]
        );
    }

    #[test]
    fn not_found_maps_to_a_distinct_message_from_not_available() {
        let not_found = map_design_source_response(Ok(LibraryResponse::NotFound))
            .expect_err("NotFound must map to Err");
        let not_available =
            map_design_source_response(Ok(LibraryResponse::DesignSourceNotAvailable))
                .expect_err("DesignSourceNotAvailable must map to Err");
        assert_ne!(
            not_found, not_available,
            "the two failure reasons must not read the same to the user"
        );
    }

    #[test]
    fn a_server_error_reply_surfaces_its_own_message() {
        let err = map_design_source_response(Ok(LibraryResponse::Error(ErrorMsg {
            code: pleochroma_net::messages::error_codes::LIBRARY_FAILED,
            message: "internal error serving the design library".to_string(),
            request_id: None,
        })))
        .expect_err("Error must map to Err");
        assert_eq!(err, "internal error serving the design library");
    }

    #[test]
    fn an_unexpected_reply_shape_is_reported_rather_than_panicking() {
        let err = map_design_source_response(Ok(LibraryResponse::FilterOptions {
            shapes: Vec::new(),
            gears: Vec::new(),
            ranges: pleochroma_net::library::AttributeRangesWire {
                ri: (0.0, 0.0),
                lw_ratio: (0.0, 0.0),
                volume: (0.0, 0.0),
                facets: (0, 0),
            },
        }))
        .expect_err("an unrelated reply shape must map to Err, not panic");
        assert!(err.contains("Unexpected reply"));
    }
}
