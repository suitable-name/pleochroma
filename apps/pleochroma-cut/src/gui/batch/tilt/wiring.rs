//! Top-level UI wiring for the tilt-curve batch: starting one, the confirm-step dialog,
//! and the Slint callback registrations. See `gui::batch::tilt` for the batch itself.

use super::{
    engine::{BatchContext, LaneShared, LiveProgress, Tally, run_batch_lanes},
    scan::spawn_missing_tilt_curve_scan,
};
use crate::{
    BatchModel, LibraryModel, MainWindow,
    bridge::{preview_render, render_thread::RenderContext},
    gui::batch::{
        batch_queue::{LanePlan, WorkQueue, local_lane_count, remote_lane_count},
        catalogue_material::load_custom_materials,
        regenerate_all::{
            ScopeKind, all_choice_label, chosen_ids, missing_among, missing_choice_label,
            next_scope_generation, scope_is_current,
        },
    },
    settings::{LiveComputeTarget, SettingsPersister, WorkerSettings},
};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, Weak};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

/// Handle returned by [`spawn_tilt_batch`]. Cancelling is cooperative: see `gui::batch::tilt`.
pub struct TiltBatchHandle {
    cancel: Arc<AtomicBool>,
}

impl TiltBatchHandle {
    /// Asks the running batch to stop before it claims another design.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The final tally reported once to `on_done` when a batch ends, for any reason.
#[derive(Debug, Clone, Copy, Default)]
pub struct TiltBatchOutcome {
    pub computed: u32,
    pub failed: u32,
    /// Designs whose geometry came from the angle table (no usable design file).
    pub angle_table: usize,
    pub cancelled: bool,
}

/// The remote and compute-target settings of [`spawn_tilt_batch`], bundled to keep that
/// function under clippy's argument-count lint (as `PreviewBatchSettings` does).
pub struct TiltBatchSettings {
    /// The remote endpoint's connection (`AppSettings::remote`), if one is configured.
    pub worker: Option<WorkerSettings>,
    pub live_compute_target: LiveComputeTarget,
    /// How many designs the batch keeps in flight on the remote at once
    /// (`AppSettings::remote_batch_lanes`); read through [`remote_lane_count`], which
    /// limits it to `1..=32`.
    pub remote_batch_lanes: u32,
}

/// Releases the batch's claim on the render context and clears the dialog's "running" flag
/// when dropped, however the batch ended.
struct BusyGuard {
    render_ctx: Arc<Mutex<RenderContext>>,
    ui_weak: Weak<MainWindow>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // A count, not a bool: the preview batch, this batch and a hi-res export
        // queue can be in flight at once, so this releases only its own claim (see
        // `RenderContext::export_active_count`).
        RenderContext::lock(&self.render_ctx).export_active_count -= 1;
        let ui_weak = self.ui_weak.clone();
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<BatchModel>().set_tilt_batch_running(false);
        });
    }
}

/// Writes the finished batch's summary line into the dialog and marks it done.
fn push_summary(ui_weak: &Weak<MainWindow>, outcome: TiltBatchOutcome) {
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        ui.global::<BatchModel>().set_tilt_summary(
            format!(
                "Computed tilt curves for {} design(s){}{}{}.",
                outcome.computed,
                if outcome.failed > 0 {
                    format!(", {} failed", outcome.failed)
                } else {
                    String::new()
                },
                super::super::angle_table_summary(outcome.angle_table),
                if outcome.cancelled {
                    " (cancelled)"
                } else {
                    ""
                }
            )
            .into(),
        );
        ui.global::<BatchModel>().set_tilt_done(true);
    });
}

/// Starts a tilt-curve batch over `entry_ids` on a background thread and returns a handle
/// that cancels it.
pub fn spawn_tilt_batch(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    settings: TiltBatchSettings,
    entry_ids: Vec<i64>,
    // This batch's id and the shared counter of the most recently started batch. The
    // final summary is pushed only while they still match (see [`start_batch`]).
    batch_id: u64,
    current_batch_id: Arc<AtomicU64>,
) -> TiltBatchHandle {
    let TiltBatchSettings {
        worker: remote_worker,
        live_compute_target,
        remote_batch_lanes,
    } = settings;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);

    thread::spawn(move || {
        RenderContext::lock(&render_ctx).export_active_count += 1;
        let _busy_guard = BusyGuard {
            render_ctx: Arc::clone(&render_ctx),
            ui_weak: ui_weak.clone(),
        };
        let _ = ui_weak
            .upgrade_in_event_loop(|ui| ui.global::<BatchModel>().set_tilt_batch_running(true));

        let design_total = u32::try_from(entry_ids.len()).unwrap_or(u32::MAX);
        let material_candidates = preview_render::ri_candidates();
        // Read ONCE for the whole batch, so every design resolves a material name against
        // the same custom-material list (`gui::batch::catalogue_material`).
        let custom_materials = load_custom_materials(&db);
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &material_candidates,
            custom_materials: &custom_materials,
            angle_table_entries: &angle_table_entries,
        };

        let queue = WorkQueue::new(entry_ids);
        let tally = Tally::default();
        let progress = Mutex::new(LiveProgress::default());

        // See `gui::batch::batch_queue::LanePlan` for what each `LiveComputeTarget` runs.
        let plan = LanePlan::for_target(live_compute_target, remote_worker.is_some());
        // Set `true` up front when no remote lane runs, so the local lane can stop as soon
        // as the queue is empty (see `gui::batch::batch_queue`).
        let remote_lane_done = AtomicBool::new(!plan.run_remote);

        // Local lanes: one less than the parallelism (see `batch_queue::local_lane_count`),
        // and none unless `plan.run_local`.
        let local_lane_total = if plan.run_local {
            u32::try_from(local_lane_count()).unwrap_or(u32::MAX)
        } else {
            0
        };
        // One remote dispatcher per design kept in flight on the remote (see
        // `batch_queue::remote_lane_count`). `LocalOnly` runs none.
        let remote_lane_total = if plan.run_remote {
            u32::try_from(remote_lane_count(remote_batch_lanes)).unwrap_or(u32::MAX)
        } else {
            0
        };

        let shared = LaneShared {
            ctx: &ctx,
            queue: &queue,
            tally: &tally,
            progress: &progress,
            cancel: &cancel_thread,
            design_total,
            local_lane_total,
            remote_lane_total,
        };

        // Lanes borrow `shared`, `queue`, `tally`, `progress` and `remote_lane_done` by
        // reference: `run_batch_lanes` joins every lane before it returns. Each lane
        // takes its own `Weak<MainWindow>` clone (see `LaneShared`).
        run_batch_lanes(
            &shared,
            plan,
            remote_worker.as_ref(),
            local_lane_total,
            &ui_weak,
            &remote_lane_done,
        );

        let outcome = TiltBatchOutcome {
            computed: tally.computed.load(Ordering::Relaxed),
            failed: tally.failed.load(Ordering::Relaxed),
            angle_table: super::super::angle_table_count(&angle_table_entries),
            cancelled: cancel_thread.load(Ordering::Relaxed),
        };
        // Skipped when a newer batch has started, so this batch cannot overwrite the new
        // batch's dialog state (see [`start_batch`]).
        if current_batch_id.load(Ordering::SeqCst) == batch_id {
            push_summary(&ui_weak, outcome);
        }
        // `_busy_guard` drops here, releasing the render-context claim and clearing
        // `tilt_batch_running`.
    });

    TiltBatchHandle { cancel }
}

/// Starts a batch for `entry_ids`.
///
/// It reads the remote endpoint and the persisted `LiveComputeTarget`
/// (`AppSettings::live_compute_target`, the live viewport's "Live Compute" setting) from
/// `settings_store`'s current snapshot. It mirrors `gui::batch::preview::wiring::start_batch`,
/// minus its preview-size settings.
///
/// Refuses (with a toast) while another batch runs. The running batch's handle must stay
/// reachable for cancel, so a second start would orphan it. Every accepted start bumps
/// `current_batch_id`, which [`spawn_tilt_batch`] compares before its final push.
fn start_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    handle_slot: &Rc<RefCell<Option<TiltBatchHandle>>>,
    current_batch_id: &Arc<AtomicU64>,
    entry_ids: Vec<i64>,
) {
    if entry_ids.is_empty() {
        return;
    }
    if ui.global::<BatchModel>().get_tilt_batch_running() {
        crate::gui::show_toast(
            ui,
            "A tilt-curve batch is already running -- wait for it to finish or cancel \
             it first.",
            "info",
        );
        return;
    }
    let snapshot = settings_store.snapshot();
    ui.global::<BatchModel>().set_tilt_single(false);
    ui.global::<BatchModel>().set_tilt_visible(true);
    ui.global::<BatchModel>().set_tilt_confirming(false);
    ui.global::<BatchModel>().set_tilt_done(false);
    ui.global::<BatchModel>().set_tilt_batch_running(true);
    ui.global::<BatchModel>().set_tilt_design_index(0);
    ui.global::<BatchModel>().set_tilt_eta(String::new().into());
    // `entry_ids.len()` is at most one catalogue's worth of rows (a few thousand in
    // the real corpus), nowhere near `i32::MAX`; saturate rather than wrap regardless.
    ui.global::<BatchModel>()
        .set_tilt_design_total(i32::try_from(entry_ids.len()).unwrap_or(i32::MAX));
    ui.global::<BatchModel>().set_tilt_local_active(0);
    ui.global::<BatchModel>().set_tilt_local_lane_total(0);
    ui.global::<BatchModel>()
        .set_tilt_remote_title(String::new().into());
    ui.global::<BatchModel>().set_tilt_remote_active(false);
    ui.global::<BatchModel>().set_tilt_remote_in_flight(0);
    ui.global::<BatchModel>()
        .set_tilt_summary(String::new().into());

    let batch_id = current_batch_id.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = spawn_tilt_batch(
        ui.as_weak(),
        Arc::clone(db),
        Arc::clone(render_ctx),
        TiltBatchSettings {
            worker: snapshot.settings.remote_worker(),
            live_compute_target: snapshot.settings.live_compute_target,
            remote_batch_lanes: snapshot.settings.remote_batch_lanes,
        },
        entry_ids,
        batch_id,
        Arc::clone(current_batch_id),
    );
    *handle_slot.borrow_mut() = Some(handle);
}

/// Opens the confirm step (`tilt_batch_dialog.slint`) for `ids`. The same flow as
/// `gui::batch::preview::offer_batch_confirmation`, on the `tilt_*` properties. A no-op for
/// an empty `ids`.
pub fn offer_batch_confirmation(ui: &MainWindow, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    // Entry ids are SQLite row ids, far below `i32::MAX`. Slint's `[int]` model has no i64
    // element type, so they are cast, as `gui::batch::preview` does.
    let ids_i32: Vec<i32> = ids
        .iter()
        .map(|&id| i32::try_from(id).unwrap_or(i32::MAX))
        .collect();
    ui.global::<BatchModel>()
        .set_tilt_offer_ids(slint::ModelRc::new(slint::VecModel::from(ids_i32)));
    ui.global::<BatchModel>()
        .set_tilt_offer_count(i32::try_from(ids.len()).unwrap_or(i32::MAX));
    ui.global::<BatchModel>().set_tilt_offer_regenerate(false);
    ui.global::<BatchModel>().set_tilt_single(false);
    ui.global::<BatchModel>().set_tilt_confirming(true);
    ui.global::<BatchModel>().set_tilt_visible(true);
}

thread_local! {
    /// What [`offer_regeneration`]'s background count needs: the vault. Set once by
    /// [`setup_tilt_batch_callbacks`]; UI-thread only.
    static SCOPE_DB: RefCell<Option<Arc<Mutex<Database>>>> = const { RefCell::new(None) };
}

/// Forgets the vault a previous main window left in [`SCOPE_DB`], so that window's database
/// connection can close. See `gui::window_release`.
pub(in crate::gui) fn release_window_state() {
    let old = SCOPE_DB.with(|db| db.borrow_mut().take());
    drop(old);
}

/// Names this module's holder in `out` while it is still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(in crate::gui) fn note_holders(out: &mut Vec<&'static str>) {
    if SCOPE_DB.with(|db| db.borrow().is_some()) {
        out.push("batch::tilt::wiring SCOPE_DB (vault)");
    }
}

/// Opens the confirm step as a REGENERATION of `ids`.
///
/// The dialog offers "missing or outdated only" and "all". The missing set is the same scan
/// the filter panel's "Compute missing tilt curves" runs (`spawn_missing_tilt_curve_scan`),
/// done off the UI thread and restricted to `ids`. Until it lands the pill reads "counting...".
/// A no-op for an empty `ids`.
pub fn offer_regeneration(ui: &MainWindow, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    offer_batch_confirmation(ui, ids);
    let model = ui.global::<BatchModel>();
    model.set_tilt_offer_regenerate(true);
    model.set_tilt_offer_missing_only(true);
    model.set_tilt_offer_missing_count(-1);
    model.set_tilt_offer_missing_ids(slint::ModelRc::default());
    model.set_tilt_offer_missing_label(missing_choice_label(None).into());
    model.set_tilt_offer_all_label(all_choice_label(ids.len()).into());

    let Some(db) = SCOPE_DB.with(|db| db.borrow().clone()) else {
        return;
    };
    let generation = next_scope_generation(ScopeKind::Tilt);
    let all = ids.to_vec();
    spawn_missing_tilt_curve_scan(ui.as_weak(), db, move |ui, missing| {
        if !scope_is_current(ScopeKind::Tilt, generation) {
            return;
        }
        let missing = missing_among(&all, &missing);
        let model = ui.global::<BatchModel>();
        model.set_tilt_offer_missing_count(i32::try_from(missing.len()).unwrap_or(i32::MAX));
        model.set_tilt_offer_missing_label(missing_choice_label(Some(missing.len())).into());
        let missing_i32: Vec<i32> = missing
            .iter()
            .map(|&id| i32::try_from(id).unwrap_or(i32::MAX))
            .collect();
        model.set_tilt_offer_missing_ids(slint::ModelRc::new(slint::VecModel::from(missing_i32)));
    });
}

/// Wires up every tilt-curve-batch-related callback on `ui`:
///
/// - `tilt_batch_cancel`/`tilt_batch_close` -- the progress dialog's Cancel/Close.
///   Close also leaves single-design mode (`tilt_single`).
/// - `tilt_batch_dismiss_offer`/`tilt_batch_generate_confirmed` -- the confirm step's
///   two buttons.
/// - `compute_tilt_curves_for_entry` -- the single-design "Generate Previews" menu entries
///   (`diagram_list.slint`, `app.slint`). It starts at once, with no confirm step.
/// - `compute_missing_tilt_curves` -- the filter panel's "Compute missing tilt curves"
///   button. It runs [`super::scan::spawn_missing_tilt_curve_scan`] first, then opens the
///   confirm step for what it finds, or toasts "nothing to do" when all is covered.
///
/// Called once at window setup. Unlike `gui::batch::preview::setup_preview_batch_callbacks`,
/// it starts no scan at startup (see [`super::scan::spawn_missing_tilt_curve_scan`]).
pub fn setup_tilt_batch_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    SCOPE_DB.with(|scope_db| *scope_db.borrow_mut() = Some(Arc::clone(db)));
    let handle: Rc<RefCell<Option<TiltBatchHandle>>> = Rc::new(RefCell::new(None));
    // Shared by every `start_batch` call site below -- see that function's own doc
    // comment for why the completion push in `spawn_tilt_batch` compares against it.
    let current_batch_id: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let handle_cancel = Rc::clone(&handle);
    ui.global::<BatchModel>().on_tilt_cancel(move || {
        if let Some(h) = handle_cancel.borrow().as_ref() {
            h.cancel();
        }
    });

    let ui_weak_close = ui.as_weak();
    ui.global::<BatchModel>().on_tilt_close(move || {
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.global::<BatchModel>().set_tilt_visible(false);
            ui.global::<BatchModel>().set_tilt_done(false);
            ui.global::<BatchModel>().set_tilt_single(false);
        }
    });

    let ui_weak_dismiss = ui.as_weak();
    ui.global::<BatchModel>().on_tilt_dismiss_offer(move || {
        if let Some(ui) = ui_weak_dismiss.upgrade() {
            ui.global::<BatchModel>().set_tilt_visible(false);
            ui.global::<BatchModel>().set_tilt_confirming(false);
        }
    });

    let db_confirm = Arc::clone(db);
    let render_ctx_confirm = Arc::clone(render_ctx);
    let settings_store_confirm = Arc::clone(settings_store);
    let handle_confirm = Rc::clone(&handle);
    let current_batch_id_confirm = Arc::clone(&current_batch_id);
    let ui_weak_confirm = ui.as_weak();
    ui.global::<BatchModel>()
        .on_tilt_generate_confirmed(move || {
            if let Some(ui) = ui_weak_confirm.upgrade() {
                let model = ui.global::<BatchModel>();
                let to_ids =
                    |ids: slint::ModelRc<i32>| ids.iter().map(i64::from).collect::<Vec<i64>>();
                let ids = chosen_ids(
                    model.get_tilt_offer_regenerate(),
                    model.get_tilt_offer_missing_only(),
                    to_ids(model.get_tilt_offer_ids()),
                    to_ids(model.get_tilt_offer_missing_ids()),
                );
                start_batch(
                    &ui,
                    &db_confirm,
                    &render_ctx_confirm,
                    &settings_store_confirm,
                    &handle_confirm,
                    &current_batch_id_confirm,
                    ids,
                );
            }
        });

    let db_entry = Arc::clone(db);
    let render_ctx_entry = Arc::clone(render_ctx);
    let settings_store_entry = Arc::clone(settings_store);
    let handle_entry = Rc::clone(&handle);
    let current_batch_id_entry = Arc::clone(&current_batch_id);
    let ui_weak_entry = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_compute_tilt_curves_for_entry(move |id: i32| {
            if let Some(ui) = ui_weak_entry.upgrade() {
                start_batch(
                    &ui,
                    &db_entry,
                    &render_ctx_entry,
                    &settings_store_entry,
                    &handle_entry,
                    &current_batch_id_entry,
                    vec![i64::from(id)],
                );
            }
        });

    let db_scan = Arc::clone(db);
    let ui_weak_scan = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_compute_missing_tilt_curves(move || {
            if let Some(ui) = ui_weak_scan.upgrade() {
                crate::gui::show_toast(
                    &ui,
                    "Scanning the catalogue for missing tilt curves...",
                    "info",
                );
                spawn_missing_tilt_curve_scan(ui.as_weak(), Arc::clone(&db_scan), |ui, ids| {
                    if ids.is_empty() {
                        crate::gui::show_toast(
                            ui,
                            "Every visible design already has tilt curves.",
                            "success",
                        );
                    } else {
                        offer_batch_confirmation(ui, &ids);
                    }
                });
            }
        });
}
