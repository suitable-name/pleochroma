//! The per-design resolve, trace and save of the RI range batch, and the lanes that pull designs
//! from a shared work queue.
//!
//! A design is judged stale by what it stores, not by a list made beforehand: each claimed
//! design is resolved, its geometry hashed with `pleochroma_editor::ri_range::planes_hash`, and
//! compared with the stored row. A row of the current `THRESHOLD_VERSION` and the same hash is
//! skipped, so a cancelled run resumes where it stopped and an edited design is traced again.

use crate::{
    MainWindow, RiBatchModel,
    gui::{
        batch::{
            batch_queue::WorkQueue, catalogue_material::catalogue_material,
            preview::target_ri_for_design, record_planes_with_frosting_for_batch,
        },
        progress_eta::{EtaEstimator, batch_eta_label},
    },
};
use pleochroma::{
    color::metrics::FinishedStone,
    geometry::{facet_finishes_with_frosted, tool::StoneGeometry},
    optics::materials::GemMaterial,
};
use pleochroma_editor::ri_range::{THRESHOLD_VERSION, compute, planes_hash};
use pleochroma_vault::{
    db::sqlite::Database,
    model::{material_match::RiPresetCandidate, ri_range::StoredRiRange},
};
use slint::{ComponentHandle as _, Weak};
use std::{
    any::Any,
    collections::BTreeSet,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
};
use tracing::warn;

/// What became of one design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// The range was traced and stored.
    Computed,
    /// The stored range is of the current version and geometry.
    UpToDate,
    /// The design could not be resolved, traced or stored.
    Failed,
    /// The batch was cancelled while this design was traced; nothing was stored.
    Cancelled,
}

/// Read-only data shared by every design of a batch.
pub(super) struct BatchContext<'a> {
    /// The vault.
    pub(super) db: &'a Mutex<Database>,
    /// The built-in materials the refractive-index pick chooses among.
    pub(super) material_candidates: &'a [RiPresetCandidate],
    /// The vault's custom materials, read once per batch.
    pub(super) custom_materials: &'a [GemMaterial],
    /// The designs whose geometry came from the angle table (a set, so a design counts once).
    pub(super) angle_table_entries: &'a Mutex<BTreeSet<i64>>,
}

/// Running totals, updated by every lane.
#[derive(Default)]
pub(super) struct Tally {
    /// Designs traced and stored.
    pub(super) computed: AtomicU32,
    /// Designs skipped because their stored range is current.
    pub(super) up_to_date: AtomicU32,
    /// Designs that could not be done.
    pub(super) failed: AtomicU32,
}

/// Live progress, shared by the lanes.
#[derive(Default)]
pub(super) struct Progress {
    /// Designs fully accounted for, by any lane.
    completed: u32,
    /// Lanes with a design claimed.
    lanes_active: u32,
    /// Fed `completed / total` on every push.
    eta: EtaEstimator,
}

/// Everything a lane borrows across the `std::thread::scope`.
pub(super) struct LaneShared<'a> {
    pub(super) ctx: &'a BatchContext<'a>,
    pub(super) queue: &'a WorkQueue<i64>,
    pub(super) tally: &'a Tally,
    pub(super) progress: &'a Mutex<Progress>,
    pub(super) cancel: &'a AtomicBool,
    /// Designs in the batch.
    pub(super) design_total: u32,
    /// Lanes spawned for the batch.
    pub(super) lane_total: u32,
}

/// `planes_hash` as the vault stores it: the same 64 bits as a signed integer.
const fn stored_hash(hash: u64) -> i64 {
    hash.cast_signed()
}

/// Whether `stored` is a range of the current thresholds for the geometry with `hash`.
pub(super) fn is_current(stored: &StoredRiRange, hash: i64) -> bool {
    stored.threshold_version == i64::from(THRESHOLD_VERSION) && stored.planes_hash == hash
}

/// Resolves, traces and stores `entry_id`'s RI range, or skips it when the stored one is current.
pub(super) fn process_entry(ctx: &BatchContext<'_>, entry_id: i64, cancel: &AtomicBool) -> Outcome {
    let full = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        guard.get_diagram_full(entry_id)
    };
    let Ok(Some(full)) = full else {
        return Outcome::Failed;
    };
    let Some((resolved, frosted)) =
        record_planes_with_frosting_for_batch(&full, ctx.angle_table_entries)
    else {
        return Outcome::Failed;
    };
    let finishes = facet_finishes_with_frosted(&resolved.planes, false, &frosted);
    let stone = FinishedStone::new(
        StoneGeometry {
            planes: resolved.planes.as_slice(),
            tools: resolved.render_tools.as_slice(),
        },
        &finishes,
    );
    let hash = stored_hash(planes_hash(stone));
    let stored = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        guard.ri_range(entry_id)
    };
    if matches!(&stored, Ok(Some(row)) if is_current(row, hash)) {
        return Outcome::UpToDate;
    }

    let chosen = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        catalogue_material(
            &guard,
            entry_id,
            &resolved,
            ctx.custom_materials,
            ctx.material_candidates,
            target_ri_for_design(&full),
        )
    };
    let n0 = match chosen {
        Ok(Some(chosen)) => f64::from(chosen.material.dispersion.n_d()),
        Ok(None) => return Outcome::Failed,
        Err(error) => {
            warn!("Could not choose the RI-range material for entry {entry_id}: {error}");
            return Outcome::Failed;
        }
    };
    let Some(range) = compute(stone, n0, cancel) else {
        return if cancel.load(Ordering::Relaxed) {
            Outcome::Cancelled
        } else {
            Outcome::Failed
        };
    };
    let row = StoredRiRange {
        entry_id,
        planes_hash: hash,
        threshold_version: i64::from(range.threshold_version),
        n_low: range.n_low,
        n_high: range.n_high,
    };
    let saved = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        guard.put_ri_range(&row)
    };
    match saved {
        Ok(()) => Outcome::Computed,
        Err(error) => {
            warn!("Could not store the RI range of entry {entry_id}: {error}");
            Outcome::Failed
        }
    }
}

/// The message of a panic payload, for the log.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Sends a snapshot of the progress to the UI thread.
pub(super) fn push_progress(ui_weak: &Weak<MainWindow>, shared: &LaneShared<'_>) {
    let (completed, lanes_active, eta_text) = {
        let mut p = shared
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        if shared.design_total > 0 {
            let fraction = f64::from(p.completed) / f64::from(shared.design_total);
            p.eta.observe(now, fraction);
        }
        let eta_text = batch_eta_label(p.completed, shared.design_total, p.eta.eta(now));
        (p.completed, p.lanes_active, eta_text)
    };
    let (design_total, lane_total) = (shared.design_total, shared.lane_total);
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<RiBatchModel>();
        model.set_eta(eta_text.into());
        model.set_design_index(completed.cast_signed());
        model.set_design_total(design_total.cast_signed());
        model.set_lanes_active(lanes_active.cast_signed());
        model.set_lanes_total(lane_total.cast_signed());
    });
}

/// Changes the busy-lane count by `delta`.
fn adjust_lanes(progress: &Mutex<Progress>, delta: i32) {
    let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
    p.lanes_active = p.lanes_active.saturating_add_signed(delta);
}

/// Counts a finished design.
fn increment_completed(progress: &Mutex<Progress>) {
    progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .completed += 1;
}

/// Runs one lane: claims designs until the queue is empty or the batch is cancelled. One bad
/// design counts as failed and never ends its lane.
fn run_lane(shared: &LaneShared<'_>, ui_weak: &Weak<MainWindow>) {
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(entry_id) = shared.queue.claim_shared() else {
            break;
        };
        adjust_lanes(shared.progress, 1);
        push_progress(ui_weak, shared);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_entry(shared.ctx, entry_id, shared.cancel)
        }));
        let outcome = result.unwrap_or_else(|payload| {
            warn!(
                "RI range computation panicked for a design: {}",
                panic_message(&*payload)
            );
            Outcome::Failed
        });

        match outcome {
            Outcome::Computed => {
                shared.tally.computed.fetch_add(1, Ordering::Relaxed);
            }
            Outcome::UpToDate => {
                shared.tally.up_to_date.fetch_add(1, Ordering::Relaxed);
            }
            Outcome::Failed => {
                shared.tally.failed.fetch_add(1, Ordering::Relaxed);
            }
            // The design is not done and not failed: it stays outstanding for the next run.
            Outcome::Cancelled => {}
        }
        adjust_lanes(shared.progress, -1);
        if outcome != Outcome::Cancelled {
            increment_completed(shared.progress);
        }
        push_progress(ui_weak, shared);
    }
}

/// Runs every lane of one batch in a `std::thread::scope` and blocks until all end.
pub(super) fn run_lanes(shared: &LaneShared<'_>, ui_weak: &Weak<MainWindow>) {
    thread::scope(|scope| {
        for _ in 0..shared.lane_total {
            let lane_ui_weak = ui_weak.clone();
            scope.spawn(move || run_lane(shared, &lane_ui_weak));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(version: i64, hash: i64) -> StoredRiRange {
        StoredRiRange {
            entry_id: 1,
            planes_hash: hash,
            threshold_version: version,
            n_low: Some(1.63),
            n_high: None,
        }
    }

    /// A row of the current version and hash is current; another version or hash is stale.
    #[test]
    fn a_row_is_current_only_for_its_version_and_geometry() {
        let version = i64::from(THRESHOLD_VERSION);
        assert!(is_current(&row(version, 7), 7));
        assert!(!is_current(&row(version, 7), 8), "the geometry changed");
        assert!(
            !is_current(&row(version + 1, 7), 7),
            "the thresholds changed"
        );
        assert!(!is_current(&row(version - 1, 7), 7), "an older version");
    }

    /// The stored hash keeps all 64 bits.
    #[test]
    fn the_stored_hash_keeps_every_bit() {
        assert_eq!(stored_hash(0), 0);
        assert_eq!(stored_hash(u64::MAX), -1);
        assert_eq!(stored_hash(1 << 63).cast_unsigned(), 1 << 63);
    }

    /// An entry that is not in the vault fails cleanly and stores nothing.
    #[test]
    fn a_missing_entry_fails_without_storing() {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            db: &db,
            material_candidates: &[],
            custom_materials: &[],
            angle_table_entries: &angle_table_entries,
        };
        let cancel = AtomicBool::new(false);
        assert_eq!(process_entry(&ctx, 999_999, &cancel), Outcome::Failed);
        let stored = db
            .lock()
            .expect("the database lock is never poisoned in a test")
            .ri_range(999_999)
            .expect("the lookup runs");
        assert!(stored.is_none());
    }
}
