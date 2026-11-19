//! The UI glue of the RI range batch: opening the dialog, counting the designs, starting and
//! cancelling a batch, and the summary. See `gui::batch::ri_range` for the batch itself.

use super::engine::{BatchContext, LaneShared, Progress, Tally, push_progress, run_lanes};
use crate::{
    MainWindow, RiBatchModel,
    bridge::{preview_render, render_thread::RenderContext},
    gui::batch::{
        batch_queue::{WorkQueue, local_lane_count},
        catalogue_material::load_custom_materials,
    },
    settings::SettingsPersister,
};
use pleochroma_editor::ri_range::THRESHOLD_VERSION;
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle as _, Weak};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};
use tracing::warn;

/// Handle of a running batch. Cancelling is cooperative: each lane checks before it claims a
/// design, and the trace checks before every pose.
struct BatchHandle {
    cancel: Arc<AtomicBool>,
}

impl BatchHandle {
    /// Asks the running batch to stop.
    fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The final tally of a batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BatchOutcome {
    /// Designs traced and stored.
    computed: u32,
    /// Designs whose stored range was current.
    up_to_date: u32,
    /// Designs that could not be done.
    failed: u32,
    /// Designs whose geometry came from the angle table.
    angle_table: usize,
    /// Whether the batch was cancelled.
    cancelled: bool,
}

/// "1 design", "12 designs".
fn designs_text(count: u32) -> String {
    format!("{count} design{}", if count == 1 { "" } else { "s" })
}

/// The summary line of a finished batch.
fn summary_text(outcome: BatchOutcome) -> String {
    let mut text = format!("Computed RI ranges for {}", designs_text(outcome.computed));
    if outcome.up_to_date > 0 {
        text.push_str(&format!(", {} already up to date", outcome.up_to_date));
    }
    if outcome.failed > 0 {
        text.push_str(&format!(", {} failed", outcome.failed));
    }
    text.push_str(&super::super::angle_table_summary(outcome.angle_table));
    if outcome.cancelled {
        text.push_str(" (cancelled)");
    }
    text.push('.');
    text
}

/// The designs of a batch: those with no current stored range first, then the rest. A design
/// listed twice is kept once.
fn queue_order(missing: Vec<i64>, current: impl IntoIterator<Item = i64>) -> Vec<i64> {
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    missing
        .into_iter()
        .chain(current)
        .filter(|id| seen.insert(*id))
        .collect()
}

/// The designs a batch goes through, and how many of them have no current range: every "Mine"
/// design that is not ignored. `None` when the vault cannot be read.
fn collect_designs(db: &Mutex<Database>) -> Option<(Vec<i64>, usize)> {
    let version = i64::from(THRESHOLD_VERSION);
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    let missing = guard
        .entries_missing_ri_range(version)
        .inspect_err(|error| warn!("Could not list the designs without an RI range: {error}"))
        .ok()?;
    let current = guard
        .mine_ri_ranges(version)
        .inspect_err(|error| warn!("Could not list the stored RI ranges: {error}"))
        .ok()?;
    drop(guard);
    let missing_count = missing.len();
    let ids = queue_order(missing, current.into_iter().map(|(id, _, _)| id));
    Some((ids, missing_count))
}

/// Releases the batch's claim on the render context and clears the dialog's "running" flag
/// when dropped, however the batch ended.
struct BusyGuard {
    render_ctx: Arc<Mutex<RenderContext>>,
    ui_weak: Weak<MainWindow>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // A count, not a bool: other batches and exports can be in flight at once.
        RenderContext::lock(&self.render_ctx).export_active_count -= 1;
        let _ = self.ui_weak.upgrade_in_event_loop(|ui| {
            ui.global::<RiBatchModel>().set_running(false);
        });
    }
}

/// Writes the finished batch's summary into the dialog, marks it done and refreshes the Material
/// Reference list.
fn push_summary(ui_weak: &Weak<MainWindow>, outcome: BatchOutcome) {
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<RiBatchModel>();
        model.set_summary(summary_text(outcome).into());
        model.set_done(true);
        crate::gui::material_reference::designs_changed();
    });
}

/// Starts a batch on a background thread and returns a handle that cancels it.
///
/// The thread lists the designs itself, so the vault is read off the UI thread. The final summary
/// is pushed only while `batch_id` is still the newest (`current_batch_id`).
fn spawn_batch(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    batch_id: u64,
    current_batch_id: Arc<AtomicU64>,
) -> BatchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = Arc::clone(&cancel);
    thread::spawn(move || {
        RenderContext::lock(&render_ctx).export_active_count += 1;
        let _busy_guard = BusyGuard {
            render_ctx: Arc::clone(&render_ctx),
            ui_weak: ui_weak.clone(),
        };
        let ids = collect_designs(&db).map_or_else(Vec::new, |(ids, _)| ids);
        let design_total = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        let lane_total = u32::try_from(local_lane_count())
            .unwrap_or(u32::MAX)
            .min(design_total.max(1));
        let material_candidates = preview_render::ri_candidates();
        // Read once for the whole batch, so every design resolves a material name against the
        // same custom-material list.
        let custom_materials = load_custom_materials(&db);
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &material_candidates,
            custom_materials: &custom_materials,
            angle_table_entries: &angle_table_entries,
        };
        let queue = WorkQueue::new(ids);
        let tally = Tally::default();
        let progress = Mutex::new(Progress::default());
        let shared = LaneShared {
            ctx: &ctx,
            queue: &queue,
            tally: &tally,
            progress: &progress,
            cancel: &cancel_thread,
            design_total,
            lane_total,
        };
        push_progress(&ui_weak, &shared);
        run_lanes(&shared, &ui_weak);

        let outcome = BatchOutcome {
            computed: tally.computed.load(Ordering::Relaxed),
            up_to_date: tally.up_to_date.load(Ordering::Relaxed),
            failed: tally.failed.load(Ordering::Relaxed),
            angle_table: super::super::angle_table_count(&angle_table_entries),
            cancelled: cancel_thread.load(Ordering::Relaxed),
        };
        if current_batch_id.load(Ordering::SeqCst) == batch_id {
            push_summary(&ui_weak, outcome);
        }
        // `_busy_guard` drops here: the render-context claim is released and `running` cleared.
    });
    BatchHandle { cancel }
}

/// Counts the designs for the confirm step off the UI thread and shows the counts, unless the
/// dialog was declined meanwhile.
fn spawn_count(ui_weak: Weak<MainWindow>, db: Arc<Mutex<Database>>) {
    thread::spawn(move || {
        let (total, missing) =
            collect_designs(&db).map_or((0, 0), |(ids, missing)| (ids.len(), missing));
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            let model = ui.global::<RiBatchModel>();
            if !model.get_confirming() {
                return;
            }
            model.set_design_count(i32::try_from(total).unwrap_or(i32::MAX));
            model.set_missing_count(i32::try_from(missing).unwrap_or(i32::MAX));
        });
    });
}

/// Opens the dialog: the confirm step with its counts, or the progress of a batch that is
/// already running.
fn open_dialog(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let model = ui.global::<RiBatchModel>();
    model.set_visible(true);
    if model.get_running() {
        model.set_confirming(false);
        return;
    }
    model.set_done(false);
    model.set_design_count(-1);
    model.set_missing_count(-1);
    model.set_confirming(true);
    spawn_count(ui.as_weak(), Arc::clone(db));
}

/// Starts a batch from the confirm step.
fn start_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    handle_slot: &Rc<RefCell<Option<BatchHandle>>>,
    current_batch_id: &Arc<AtomicU64>,
) {
    let model = ui.global::<RiBatchModel>();
    if model.get_running() {
        return;
    }
    let total = model.get_design_count().max(0);
    model.set_confirming(false);
    model.set_done(false);
    model.set_running(true);
    model.set_design_index(0);
    model.set_design_total(total);
    model.set_eta(String::new().into());
    model.set_lanes_active(0);
    model.set_lanes_total(0);
    model.set_summary(String::new().into());
    let batch_id = current_batch_id.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = spawn_batch(
        ui.as_weak(),
        Arc::clone(db),
        Arc::clone(render_ctx),
        batch_id,
        Arc::clone(current_batch_id),
    );
    *handle_slot.borrow_mut() = Some(handle);
}

/// Wires every `RiBatchModel` callback: `open_batch`, `start`, `cancel` and `close`.
///
/// `cancel` stops a running batch (what is finished stays stored) and otherwise declines the
/// confirm step. Called once at window setup. The batch computes locally; the settings store is
/// not needed.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    _settings_store: &Arc<SettingsPersister>,
) {
    let handle: Rc<RefCell<Option<BatchHandle>>> = Rc::new(RefCell::new(None));
    // The completion push compares against it, so a finished older batch never overwrites the
    // dialog of a newer one.
    let current_batch_id: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    // The Material Reference window's list of designs reads the same vault.
    crate::gui::material_reference::set_designs_database(db);
    let model = ui.global::<RiBatchModel>();

    let ui_weak_open = ui.as_weak();
    let db_open = Arc::clone(db);
    model.on_open_batch(move || {
        if let Some(ui) = ui_weak_open.upgrade() {
            open_dialog(&ui, &db_open);
        }
    });

    let ui_weak_start = ui.as_weak();
    let db_start = Arc::clone(db);
    let render_ctx_start = Arc::clone(render_ctx);
    let handle_start = Rc::clone(&handle);
    let current_batch_id_start = Arc::clone(&current_batch_id);
    model.on_start(move || {
        if let Some(ui) = ui_weak_start.upgrade() {
            start_batch(
                &ui,
                &db_start,
                &render_ctx_start,
                &handle_start,
                &current_batch_id_start,
            );
        }
    });

    let ui_weak_cancel = ui.as_weak();
    let handle_cancel = Rc::clone(&handle);
    model.on_cancel(move || {
        let Some(ui) = ui_weak_cancel.upgrade() else {
            return;
        };
        let model = ui.global::<RiBatchModel>();
        if model.get_running() {
            if let Some(handle) = handle_cancel.borrow().as_ref() {
                handle.cancel();
            }
        } else {
            model.set_visible(false);
            model.set_confirming(false);
        }
    });

    let ui_weak_close = ui.as_weak();
    model.on_close(move || {
        if let Some(ui) = ui_weak_close.upgrade() {
            let model = ui.global::<RiBatchModel>();
            model.set_visible(false);
            model.set_done(false);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_designs_text_counts_one_and_many() {
        assert_eq!(designs_text(0), "0 designs");
        assert_eq!(designs_text(1), "1 design");
        assert_eq!(designs_text(12), "12 designs");
    }

    #[test]
    fn the_summary_names_what_happened() {
        let plain = BatchOutcome {
            computed: 12,
            ..BatchOutcome::default()
        };
        assert_eq!(summary_text(plain), "Computed RI ranges for 12 designs.");
        let full = BatchOutcome {
            computed: 1,
            up_to_date: 3,
            failed: 2,
            angle_table: 0,
            cancelled: true,
        };
        assert_eq!(
            summary_text(full),
            "Computed RI ranges for 1 design, 3 already up to date, 2 failed (cancelled)."
        );
    }

    #[test]
    fn the_queue_puts_designs_without_a_current_range_first_and_keeps_each_once() {
        assert_eq!(queue_order(vec![5, 2], [2, 9, 5, 7]), vec![5, 2, 9, 7]);
        assert_eq!(
            queue_order(Vec::new(), Vec::<i64>::new()),
            Vec::<i64>::new()
        );
    }

    /// An empty vault lists no design.
    #[test]
    fn collect_designs_reads_the_vault_lists() {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let (ids, missing) = collect_designs(&db).expect("an empty vault lists");
        assert!(ids.is_empty());
        assert_eq!(missing, 0);
    }
}
