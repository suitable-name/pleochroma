//! The RI range worker: the traced range of the open design, off the UI thread.
//!
//! The verdict's worker (`gui::editor::verdict`) hands every solve and material change of the
//! editor to [`note_solved`] and [`design_changed`]. The geometric hint shows at once
//! ([`super::present`]); the trace starts once changes pause for [`REQUEST_DEBOUNCE`], runs on a
//! thread of its own and lands back on the UI thread. A newer change cancels the run in flight.
//!
//! The cache is keyed by `planes_hash` and `THRESHOLD_VERSION` (`pleochroma_editor::ri_range::
//! cache_key`) and by the design's own index, because the range is judged against the design at
//! that index. A design that is a library entry also has its range stored (`put_ri_range`), for
//! "Your designs that work here".
//!
//! # State
//!
//! A `thread_local!` [`STATE`] on the UI thread, like the verdict's slot: the workers only hand a
//! result back through the event loop, and a result whose serial is not current is dropped.

use super::present;
use crate::{
    MainWindow, RiChartModel, RiRangeModel, bridge::render_thread::RenderContext,
    settings::SettingsPersister,
};
use pleochroma::{
    color::metrics::FinishedStone,
    geometry::{StoneGeometry, meet_solver::SolvedTier},
};
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    ri_range::{self, RiRange},
    solve_policy::design_to_gpu_planes_from_solved,
};
use pleochroma_vault::{db::sqlite::Database, model::ri_range::StoredRiRange};
use slint::{ComponentHandle, Timer, TimerMode, Weak};
use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// How long a change waits for a newer one before the trace starts: a drag replans many times a
/// second, and a trace costs about 350 pose evaluations.
const REQUEST_DEBOUNCE: Duration = Duration::from_millis(400);

/// How many traced ranges the cache keeps.
const CACHE_CAPACITY: usize = 12;

/// What a waiter gets when the range of the current design exists.
type Waiter = Box<dyn FnOnce(&MainWindow, Arc<RiRange>)>;

/// The design as the last solve left it, copied so the worker can own it.
pub(super) struct Snapshot {
    /// The design.
    pub(super) design: Design,
    /// The masts of its solve.
    pub(super) solved: Vec<SolvedTier>,
    /// The design's effective refractive index.
    pub(super) n0: f64,
    /// The library row the design belongs to, once it has one.
    pub(super) entry_id: Option<i64>,
}

/// What the UI thread remembers of the range.
#[derive(Default)]
struct State {
    /// The design the next trace is about; `None` without a solved design.
    snapshot: Option<Arc<Snapshot>>,
    /// Bumped by every change. A result with another serial is dropped.
    serial: u64,
    /// The range of the design as it stands, once it is traced.
    latest: Option<Arc<RiRange>>,
    /// Stops the run in flight.
    cancel: Option<Arc<AtomicBool>>,
    /// Callers waiting for the range of the current design.
    waiting: Vec<Waiter>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Holds the trace back until changes stop arriving for [`REQUEST_DEBOUNCE`].
    static DEBOUNCE: Timer = Timer::default();
    /// The library, to store the range of a design that is an entry.
    static DB: RefCell<Option<Arc<Mutex<Database>>>> = const { RefCell::new(None) };
}

/// Traced ranges, newest last; one entry per `(planes, version, index)`.
static CACHE: Mutex<Vec<(u64, Arc<RiRange>)>> = Mutex::new(Vec::new());

/// Registers the chip's click and keeps the library for storing ranges.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    _render_ctx: &Arc<Mutex<RenderContext>>,
    _settings_store: &Arc<SettingsPersister>,
) {
    DB.with(|cell| *cell.borrow_mut() = Some(Arc::clone(db)));
    let ui_weak = ui.as_weak();
    ui.global::<RiRangeModel>().on_chip_clicked(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<RiChartModel>().invoke_open_chart();
        }
    });
}

/// Lets go of what a previous main window left here: stops the debounce, cancels a trace in
/// flight, drops the waiting callers and the remembered design, and drops the library handle, so
/// that window's database file can close. See `gui::window_release`.
pub(in crate::gui) fn release_window_state() {
    DEBOUNCE.with(Timer::stop);
    let old = STATE.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    if let Some(cancel) = &old.cancel {
        cancel.store(true, Ordering::Relaxed);
    }
    drop(old);
    let db = DB.with(|cell| cell.borrow_mut().take());
    drop(db);
}

/// Whether the library handle is still held. UI harness only.
#[cfg(feature = "ui-harness")]
pub(in crate::gui) fn holds_database() -> bool {
    DB.with(|cell| cell.borrow().is_some())
}

/// The newest result for the design as it is solved now.
///
/// `None` while it is computing, and without a design.
pub(in crate::gui) fn latest() -> Option<Arc<RiRange>> {
    STATE.with(|cell| cell.borrow().latest.clone())
}

/// Runs `done` on the UI thread once a result for the current design exists (at once if it does).
pub(in crate::gui) fn when_ready(
    ui: &MainWindow,
    done: impl FnOnce(&MainWindow, Arc<RiRange>) + 'static,
) {
    let Some(range) = latest() else {
        STATE.with(|cell| cell.borrow_mut().waiting.push(Box::new(done)));
        return;
    };
    done(ui, range);
}

/// Keeps the design of the solve that just landed; [`design_changed`] acts on it.
///
/// `solved` is `None` for a design that did not solve, and a design without tiers is no design.
pub(in crate::gui) fn note_solved(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n_d: f64,
    entry_id: Option<i64>,
) {
    let snapshot = snapshot_of(design, solved, n_d, entry_id);
    STATE.with(|cell| cell.borrow_mut().snapshot = snapshot);
}

/// The open design has gone (or has no tiers): hides the chip and forgets the range.
pub(in crate::gui) fn design_cleared(ui: &MainWindow) {
    STATE.with(|cell| cell.borrow_mut().snapshot = None);
    design_changed(ui);
}

/// Called after every solve and material change: shows the hint and starts the trace once
/// changes pause.
pub(in crate::gui) fn design_changed(ui: &MainWindow) {
    let (serial, snapshot) = STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        if let Some(cancel) = state.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        state.serial += 1;
        state.latest = None;
        (state.serial, state.snapshot.clone())
    });
    DEBOUNCE.with(Timer::stop);
    let Some(snapshot) = snapshot else {
        present::hide(ui);
        return;
    };
    present::show_hint(ui, &snapshot.design, &snapshot.solved);
    let ui_weak = ui.as_weak();
    DEBOUNCE.with(|timer| {
        timer.start(TimerMode::SingleShot, REQUEST_DEBOUNCE, move || {
            start(ui_weak.clone(), serial, Arc::clone(&snapshot));
        });
    });
}

/// The snapshot of a solve, or `None` for a design that gives nothing to trace.
fn snapshot_of(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    n0: f64,
    entry_id: Option<i64>,
) -> Option<Arc<Snapshot>> {
    let solved = solved.filter(|_| !design.tiers.is_empty())?;
    Some(Arc::new(Snapshot {
        design: design.clone(),
        solved: solved.to_vec(),
        n0,
        entry_id,
    }))
}

/// Starts the trace of `snapshot` on a thread of its own.
fn start(ui_weak: Weak<MainWindow>, serial: u64, snapshot: Arc<Snapshot>) {
    let cancel = Arc::new(AtomicBool::new(false));
    STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        if state.serial == serial {
            state.cancel = Some(Arc::clone(&cancel));
        }
    });
    let db = DB.with(|cell| cell.borrow().clone());
    let spawned = thread::Builder::new()
        .name("ri-range".to_owned())
        .spawn(move || {
            // A panic inside the trace must not take the window with it; the hint stays.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                trace(&snapshot, &cancel, db.as_deref())
            }));
            let range = outcome.ok().flatten();
            let _ = ui_weak.upgrade_in_event_loop(move |ui| landed(&ui, serial, range));
        });
    if let Err(error) = spawned {
        tracing::warn!("the RI range could not be traced: {error}");
    }
}

/// A trace's result, back on the UI thread. Dropped when `serial` is no longer current.
fn landed(ui: &MainWindow, serial: u64, range: Option<Arc<RiRange>>) {
    let Some(range) = range else {
        return;
    };
    let waiting = STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        if state.serial != serial {
            return None;
        }
        state.latest = Some(Arc::clone(&range));
        state.cancel = None;
        Some(std::mem::take(&mut state.waiting))
    });
    let Some(waiting) = waiting else {
        return;
    };
    present::show_traced(ui, &range);
    crate::gui::editor::verdict::apply_small_tilt(ui);
    for done in waiting {
        done(ui, Arc::clone(&range));
    }
}

/// Traces `snapshot` (or takes the cached range) and stores it for a library entry.
///
/// `None` when `cancel` stopped it.
fn trace(
    snapshot: &Snapshot,
    cancel: &AtomicBool,
    db: Option<&Mutex<Database>>,
) -> Option<Arc<RiRange>> {
    with_stone(&snapshot.design, &snapshot.solved, |stone| {
        let slot = cache_slot(ri_range::cache_key(stone), snapshot.n0);
        let range = cache_get(slot).or_else(|| {
            let range = Arc::new(ri_range::compute(stone, snapshot.n0, cancel)?);
            cache_put(slot, Arc::clone(&range));
            Some(range)
        })?;
        if let (Some(db), Some(entry_id)) = (db, snapshot.entry_id) {
            persist(db, entry_id, ri_range::planes_hash(stone), &range);
        }
        Some(range)
    })
}

/// Hands `f` the finished stone the render shows (concave tools and frosted facets included).
fn with_stone<R>(
    design: &Design,
    solved: &[SolvedTier],
    f: impl FnOnce(FinishedStone<'_>) -> R,
) -> R {
    let planes = design.planes_from_solved(solved);
    let gpu_planes = design_to_gpu_planes_from_solved(design, solved);
    let tools = design.tools_for_scoring(&planes);
    let finishes = design.facet_finishes_from_solved(solved, &gpu_planes);
    f(FinishedStone::new(
        StoneGeometry {
            planes: &gpu_planes,
            tools: &tools,
        },
        &finishes,
    ))
}

/// Stores the range of library entry `entry_id` (the hash bits as the library keeps them).
fn persist(db: &Mutex<Database>, entry_id: i64, planes_hash: u64, range: &RiRange) {
    let stored = StoredRiRange {
        entry_id,
        planes_hash: i64::from_ne_bytes(planes_hash.to_ne_bytes()),
        threshold_version: i64::from(range.threshold_version),
        n_low: range.n_low,
        n_high: range.n_high,
    };
    let db = db.lock().unwrap_or_else(PoisonError::into_inner);
    if let Err(error) = db.put_ri_range(&stored) {
        tracing::debug!("the RI range was not stored: {error:#}");
    }
}

/// The cache slot of a stone's key and the design's own index.
fn cache_slot(key: u64, n0: f64) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    n0.to_bits().hash(&mut hasher);
    hasher.finish()
}

/// The cached range of `slot`.
fn cache_get(slot: u64) -> Option<Arc<RiRange>> {
    let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache
        .iter()
        .find(|(key, _)| *key == slot)
        .map(|(_, range)| Arc::clone(range))
}

/// Remembers `range` under `slot`, dropping the oldest beyond [`CACHE_CAPACITY`].
fn cache_put(slot: u64, range: Arc<RiRange>) {
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache.retain(|(key, _)| *key != slot);
    cache.push((slot, range));
    if cache.len() > CACHE_CAPACITY {
        cache.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use pleochroma_editor::ri_range::{RiGridRow, THRESHOLD_VERSION};

    fn brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    /// A range with the given low bound and nothing else.
    fn range_low(n_low: f64) -> Arc<RiRange> {
        let row = RiGridRow {
            n: 1.5,
            windowing_table_up: 0.0,
            windowing_tilt: 0.0,
            brilliance_tilt: 0.0,
            extinction_tilt: 0.0,
        };
        Arc::new(RiRange {
            n0: 1.5,
            own: row,
            rows: vec![row],
            n_low: Some(n_low),
            n_high: None,
            threshold_version: THRESHOLD_VERSION,
        })
    }

    #[test]
    fn a_design_without_tiers_or_without_a_solve_gives_no_snapshot() {
        let mut design = brilliant();
        let solved = design.solve().expect("every tier is pinned");
        assert!(snapshot_of(&design, Some(&solved), 1.76, None).is_some());
        assert!(snapshot_of(&design, None, 1.76, None).is_none());
        design.tiers.clear();
        assert!(snapshot_of(&design, Some(&solved), 1.76, None).is_none());
    }

    #[test]
    fn the_snapshot_carries_the_index_and_the_library_row() {
        let design = brilliant();
        let solved = design.clone().solve().expect("every tier is pinned");
        let snapshot = snapshot_of(&design, Some(&solved), 1.768, Some(7)).expect("a snapshot");
        assert_eq!(snapshot.entry_id, Some(7));
        assert!((snapshot.n0 - 1.768).abs() < f64::EPSILON);
        assert_eq!(snapshot.solved.len(), solved.len());
    }

    #[test]
    fn the_cache_slot_follows_the_key_and_the_design_index() {
        let slot = cache_slot(42, 1.768);
        assert_eq!(slot, cache_slot(42, 1.768));
        assert_ne!(slot, cache_slot(43, 1.768));
        assert_ne!(slot, cache_slot(42, 1.54));
    }

    #[test]
    fn the_cache_returns_what_was_put_and_forgets_the_oldest() {
        // Slots far from anything another test uses.
        let base = 0xFEED_0000_u64;
        let capacity = u64::try_from(CACHE_CAPACITY).expect("a small count");
        for offset in 0..=capacity {
            cache_put(base + offset, range_low(1.6));
        }
        assert!(cache_get(base + capacity).is_some());
        assert!(cache_get(base).is_none(), "the oldest slot went first");
        let again = range_low(1.7);
        cache_put(base + 1, Arc::clone(&again));
        assert_eq!(cache_get(base + 1).as_deref(), Some(&*again));
    }

    #[test]
    fn a_cancelled_trace_gives_nothing_and_caches_nothing() {
        let mut design = brilliant();
        design.material.name = Some("Sapphire".to_owned());
        let solved = design.solve().expect("every tier is pinned");
        let snapshot = snapshot_of(&design, Some(&solved), 1.768, None).expect("a snapshot");
        let cancel = AtomicBool::new(true);
        assert!(trace(&snapshot, &cancel, None).is_none());
    }

    #[test]
    fn latest_is_empty_until_a_result_for_the_current_design_lands() {
        STATE.with(|cell| *cell.borrow_mut() = State::default());
        assert!(latest().is_none());
        let range = range_low(1.63);
        STATE.with(|cell| cell.borrow_mut().latest = Some(Arc::clone(&range)));
        assert_eq!(latest().as_deref(), Some(&*range));
        STATE.with(|cell| *cell.borrow_mut() = State::default());
    }
}
