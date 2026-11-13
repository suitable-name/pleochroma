//! [`ActivityRegistry`] is the single place every long-running operation registers itself.
//!
//! Every long-running background operation
//! (auto-solve, Deep Solve, Optimize, Retarget optimize, Snapshot compare, tilt
//! curve/video export, trace generation) registers itself while it runs. The Slint
//! side is [`crate::ActivityModel`] (`ui/models/activity.slint`); this module is its
//! only writer.
//!
//! # Two layers, for testability
//!
//! [`ActivityList`] is the pure bookkeeping (start/progress/finish over a plain `Vec`,
//! no Slint handle), so tests can run without a windowing backend. [`CancelMap`] is
//! the matching pure bookkeeping for "which closure does cancelling activity N
//! invoke." [`ActivityRegistry`] wraps both with the [`slint::Weak`] handle that
//! pushes [`crate::ActivityItem`] rows and the one-second elapsed-time ticker; only
//! the app exercises this outer layer.
//!
//! # Why `started_ms`/`now_ms` are a per-registry epoch, not wall-clock time
//!
//! [`crate::ActivityItem::started_ms`] and [`crate::ActivityModel::now_ms`] are Slint
//! `int` (`i32`) properties; Unix-epoch milliseconds would overflow them within about
//! 25 days. Values are measured from [`ActivityRegistry::epoch`] instead, which keeps
//! them in `i32` for a whole process lifetime. The `.slint` side only shows the
//! difference between the two, so the epoch itself is invisible to callers.

use crate::{ActivityItem, ActivityModel, MainWindow};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak as RcWeak},
    time::{Duration, Instant},
};

/// How often [`ActivityRegistry`]'s elapsed-time ticker refreshes
/// [`crate::ActivityModel::now_ms`] while an activity runs. Coarse on purpose: it only
/// drives an "N s elapsed" label, and must not compete with the 16 ms frame budget.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// A negative "no fraction known" marker for [`ActivityEntry::progress`]/
/// [`crate::ActivityItem::progress`] -- see that field's own doc comment
/// (`ui/models/activity.slint`) for why most callers pass this rather than a real
/// fraction.
pub(in crate::gui::editor) const INDETERMINATE: f32 = -1.0;

/// One running activity's pure bookkeeping -- everything [`crate::ActivityItem`]
/// needs, plus the internal `id` [`ActivityList`] uses to find it again.
#[derive(Debug, Clone, PartialEq)]
struct ActivityEntry {
    id: u64,
    kind: String,
    label: String,
    progress: f32,
    cancellable: bool,
    started_ms: i64,
}

/// The pure, Slint-free half of [`ActivityRegistry`] -- see the module doc comment.
/// Entries are kept in START order (a plain `Vec` push/retain, never sorted), which
/// is also the order [`ActivityRegistry::sync_model`] hands to
/// [`crate::ActivityModel::activities`], so the status strip lists the
/// longest-running activity first.
#[derive(Debug, Default)]
struct ActivityList {
    next_id: u64,
    entries: Vec<ActivityEntry>,
}

impl ActivityList {
    /// Registers a new activity and returns its id. Ids are never reused, even after
    /// `finish`, so a stale id can never match a newer activity.
    fn start(&mut self, kind: String, label: String, cancellable: bool, started_ms: i64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(ActivityEntry {
            id,
            kind,
            label,
            progress: INDETERMINATE,
            cancellable,
            started_ms,
        });
        id
    }

    /// Updates `id`'s progress fraction in place. `false` (a silent no-op for the
    /// caller) if `id` has already finished -- a progress tick racing a completion
    /// is expected, never an error.
    fn set_progress(&mut self, id: u64, progress: f32) -> bool {
        self.entries
            .iter_mut()
            .find(|e| e.id == id)
            .map(|e| e.progress = progress)
            .is_some()
    }

    /// Removes `id`. `false` if it was already gone (a duplicate `finish`, or one
    /// racing a cancel that already removed it) -- again a silent no-op, never an
    /// error, matching [`Self::set_progress`]'s own convention.
    fn finish(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    /// The current entries, in start order -- see the struct's own doc comment.
    fn entries(&self) -> &[ActivityEntry] {
        &self.entries
    }
}

/// The pure "which closure does cancelling activity N invoke" bookkeeping --
/// `BTreeMap`, not `HashMap` (house rule: no hash-map iteration in a decision path;
/// this app's activity count is always tiny, but the rule is unconditional).
#[derive(Default)]
struct CancelMap {
    entries: BTreeMap<u64, Box<dyn Fn()>>,
}

impl CancelMap {
    /// Registers `cancel` for `id`, replacing anything already registered for that
    /// id (cannot happen in practice -- ids are never reused, see
    /// [`ActivityList::start`] -- kept total rather than panicking on a
    /// same-id double-register).
    fn insert(&mut self, id: u64, cancel: Box<dyn Fn()>) {
        self.entries.insert(id, cancel);
    }

    /// Drops `id`'s cancel closure, if any -- called once an activity finishes, so
    /// a stray later `cancel` call cannot reach back into a closure whose
    /// underlying handle (a `DeepSolveHandle`, say) may itself have been dropped.
    fn remove(&mut self, id: u64) {
        self.entries.remove(&id);
    }

    /// Invokes `id`'s registered cancel closure, if any -- a no-op for an unknown
    /// or non-cancellable id, exactly like [`ActivityList::set_progress`]/
    /// [`ActivityList::finish`]'s own "unknown id is harmless" convention.
    fn invoke(&self, id: u64) {
        if let Some(cancel) = self.entries.get(&id) {
            cancel();
        }
    }
}

/// Owns [`ActivityList`]/[`CancelMap`] plus the Slint-facing half: pushing
/// [`crate::ActivityItem`] rows into [`crate::ActivityModel`] and ticking
/// [`crate::ActivityModel::now_ms`] once a second while anything is running. See the
/// module doc comment for the two-layer split.
pub(in crate::gui::editor) struct ActivityRegistry {
    list: RefCell<ActivityList>,
    cancels: RefCell<CancelMap>,
    ui: Weak<MainWindow>,
    /// See the module doc comment, "Why `started_ms`/`now_ms` are a per-registry
    /// epoch".
    epoch: Instant,
    ticker: slint::Timer,
    /// Weak handle to this registry, for the ticker closure. A strong `Rc` here would
    /// form a reference cycle through `ticker`, which this struct owns.
    self_weak: RcWeak<Self>,
}

impl ActivityRegistry {
    /// Builds a new, empty registry bound to `ui`. One instance is constructed in
    /// [`super::setup_editor_callbacks`] and shared (via `Rc::clone`) with every
    /// module that registers a long-running activity -- see each `start` call site.
    pub(in crate::gui::editor) fn new(ui: &MainWindow) -> Rc<Self> {
        let registry = Rc::new_cyclic(|weak_self| Self {
            list: RefCell::new(ActivityList::default()),
            cancels: RefCell::new(CancelMap::default()),
            ui: ui.as_weak(),
            epoch: Instant::now(),
            ticker: slint::Timer::default(),
            self_weak: weak_self.clone(),
        });
        ui.global::<ActivityModel>().on_cancel({
            let registry = Rc::clone(&registry);
            move |id| {
                let Ok(id) = u64::try_from(id) else {
                    return;
                };
                registry.cancels.borrow().invoke(id);
            }
        });
        Self::wire_external_bridge(&registry, ui);
        registry
    }

    /// Wires the `start_external`/`progress_external`/`finish_external` callbacks of
    /// `ActivityModel` (see `ui/models/activity.slint`). Takes `&Rc<Self>` so it can
    /// run right after `Rc::new_cyclic` in [`Self::new`].
    fn wire_external_bridge(registry: &Rc<Self>, ui: &MainWindow) {
        ui.global::<ActivityModel>().on_start_external({
            let registry = Rc::clone(registry);
            move |kind, label, cancellable| {
                let id = registry.start_external(kind.to_string(), label.to_string(), cancellable);
                i32::try_from(id).unwrap_or(i32::MAX)
            }
        });
        ui.global::<ActivityModel>().on_progress_external({
            let registry = Rc::clone(registry);
            move |id, progress| {
                if let Ok(id) = u64::try_from(id) {
                    registry.progress(id, progress);
                }
            }
        });
        ui.global::<ActivityModel>().on_finish_external({
            let registry = Rc::clone(registry);
            move |id| {
                if let Ok(id) = u64::try_from(id) {
                    registry.finish(id);
                }
            }
        });
    }

    /// Body of the `start_external` callback. A Slint callback carries only plain data,
    /// so a cancellable external activity cannot hold a Rust closure; its registered
    /// cancel instead invokes `ActivityModel.external_cancel(id)` (see
    /// `ui/models/activity.slint`).
    pub(in crate::gui::editor) fn start_external(
        &self,
        kind: String,
        label: String,
        cancellable: bool,
    ) -> u64 {
        let started_ms = self.elapsed_ms();
        let was_empty = self.list.borrow().entries().is_empty();
        let id = self
            .list
            .borrow_mut()
            .start(kind, label, cancellable, started_ms);
        if cancellable {
            let ui = self.ui.clone();
            self.cancels.borrow_mut().insert(
                id,
                Box::new(move || {
                    if let Some(ui) = ui.upgrade() {
                        ui.global::<ActivityModel>()
                            .invoke_external_cancel(i32::try_from(id).unwrap_or(i32::MAX));
                    }
                }),
            );
        }
        self.sync_model();
        if was_empty {
            self.start_ticker();
        }
        id
    }

    /// Milliseconds since [`Self::epoch`], saturating on overflow. See the module doc
    /// for why `i32` is never approached in practice.
    fn elapsed_ms(&self) -> i64 {
        i64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    /// Registers a new activity, pushes it into [`crate::ActivityModel`], and starts
    /// the elapsed-time ticker if it is the first one running. `cancel` is `None` for
    /// an activity with no cancellation handle, which hides its Cancel button;
    /// `Some` is called when the user cancels.
    pub(in crate::gui::editor) fn start(
        &self,
        kind: impl Into<String>,
        label: impl Into<String>,
        cancel: Option<Box<dyn Fn()>>,
    ) -> u64 {
        let started_ms = self.elapsed_ms();
        let was_empty = self.list.borrow().entries().is_empty();
        let id =
            self.list
                .borrow_mut()
                .start(kind.into(), label.into(), cancel.is_some(), started_ms);
        if let Some(cancel) = cancel {
            self.cancels.borrow_mut().insert(id, cancel);
        }
        self.sync_model();
        if was_empty {
            self.start_ticker();
        }
        id
    }

    /// Updates `id`'s progress fraction (see [`crate::ActivityItem::progress`]'s own
    /// doc comment for the `-1.0` "indeterminate" convention) and re-syncs the
    /// model. A no-op if `id` already finished.
    pub(in crate::gui::editor) fn progress(&self, id: u64, progress: f32) {
        if self.list.borrow_mut().set_progress(id, progress) {
            self.sync_model();
        }
    }

    /// Removes `id`, drops its cancel closure, and re-syncs the model. Stops the
    /// elapsed-time ticker once the list is empty.
    pub(in crate::gui::editor) fn finish(&self, id: u64) {
        self.cancels.borrow_mut().remove(id);
        if self.list.borrow_mut().finish(id) {
            self.sync_model();
        }
        if self.list.borrow().entries().is_empty() {
            self.ticker.stop();
        }
    }

    /// Starts the one-second elapsed-time ticker. Called only when the list goes from
    /// empty to non-empty, so the ticker is not restarted for every activity.
    fn start_ticker(&self) {
        let registry = self.self_weak.clone();
        self.ticker
            .start(slint::TimerMode::Repeated, TICK_INTERVAL, move || {
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                if registry.list.borrow().entries().is_empty() {
                    registry.ticker.stop();
                    return;
                }
                registry.sync_model();
            });
    }

    /// Rebuilds [`crate::ActivityModel::activities`] and `now_ms` from the current
    /// entries. The count is always a handful, so rebuilding the whole model on each
    /// change is cheap.
    fn sync_model(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let now_ms = self.elapsed_ms();
        let items: Vec<ActivityItem> = self
            .list
            .borrow()
            .entries()
            .iter()
            .map(|e| ActivityItem {
                id: i32::try_from(e.id).unwrap_or(i32::MAX),
                kind: e.kind.as_str().into(),
                label: e.label.as_str().into(),
                progress: e.progress,
                cancellable: e.cancellable,
                started_ms: i32::try_from(e.started_ms).unwrap_or(i32::MAX),
            })
            .collect();
        let model = ui.global::<ActivityModel>();
        model.set_activities(ModelRc::new(VecModel::from(items)));
        model.set_now_ms(i32::try_from(now_ms).unwrap_or(i32::MAX));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ActivityList: ordering, progress, finish ---

    #[test]
    fn start_returns_monotonically_increasing_ids() {
        let mut list = ActivityList::default();
        let a = list.start("solve".into(), "Solving".into(), true, 0);
        let b = list.start("optimize".into(), "Optimizing".into(), true, 10);
        let c = list.start("deep_solve".into(), "Deep Solve".into(), false, 20);
        assert_eq!((a, b, c), (1, 2, 3));
    }

    #[test]
    fn entries_are_kept_in_start_order() {
        let mut list = ActivityList::default();
        list.start("a".into(), "A".into(), false, 0);
        list.start("b".into(), "B".into(), false, 0);
        list.start("c".into(), "C".into(), false, 0);
        let kinds: Vec<&str> = list.entries().iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["a", "b", "c"]);
    }

    #[test]
    fn finishing_the_middle_entry_preserves_the_order_of_the_rest() {
        let mut list = ActivityList::default();
        let a = list.start("a".into(), "A".into(), false, 0);
        let b = list.start("b".into(), "B".into(), false, 0);
        let c = list.start("c".into(), "C".into(), false, 0);
        assert!(list.finish(b));
        let ids: Vec<u64> = list.entries().iter().map(|e| e.id).collect();
        assert_eq!(ids, [a, c]);
    }

    #[test]
    fn finish_of_an_unknown_id_is_a_harmless_no_op() {
        let mut list = ActivityList::default();
        list.start("a".into(), "A".into(), false, 0);
        assert!(!list.finish(999));
        assert_eq!(list.entries().len(), 1, "the real entry must be untouched");
    }

    #[test]
    fn set_progress_updates_the_matching_entry_only() {
        let mut list = ActivityList::default();
        let a = list.start("a".into(), "A".into(), false, 0);
        let b = list.start("b".into(), "B".into(), false, 0);
        assert!(list.set_progress(b, 0.5));
        let entry_a = list.entries().iter().find(|e| e.id == a).unwrap();
        let entry_b = list.entries().iter().find(|e| e.id == b).unwrap();
        assert_eq!(
            entry_a.progress, INDETERMINATE,
            "untouched entry must stay indeterminate"
        );
        assert_eq!(entry_b.progress, 0.5);
    }

    #[test]
    fn set_progress_on_an_unknown_id_returns_false() {
        let mut list = ActivityList::default();
        list.start("a".into(), "A".into(), false, 0);
        assert!(!list.set_progress(999, 0.5));
    }

    #[test]
    fn ids_are_never_reused_even_after_a_finish() {
        let mut list = ActivityList::default();
        let a = list.start("a".into(), "A".into(), false, 0);
        list.finish(a);
        let b = list.start("b".into(), "B".into(), false, 0);
        assert_ne!(a, b, "a finished id must never be handed out again");
    }

    // --- CancelMap: cancel wiring ---

    #[test]
    fn invoking_a_registered_cancel_calls_the_closure_exactly_once() {
        let calls = Rc::new(RefCell::new(0));
        let mut map = CancelMap::default();
        map.insert(
            1,
            Box::new({
                let calls = Rc::clone(&calls);
                move || *calls.borrow_mut() += 1
            }),
        );
        map.invoke(1);
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn invoking_an_unregistered_id_does_nothing() {
        let map = CancelMap::default();
        map.invoke(42); // must not panic
    }

    #[test]
    fn removing_a_cancel_stops_it_from_firing() {
        let calls = Rc::new(RefCell::new(0));
        let mut map = CancelMap::default();
        map.insert(
            1,
            Box::new({
                let calls = Rc::clone(&calls);
                move || *calls.borrow_mut() += 1
            }),
        );
        map.remove(1);
        map.invoke(1);
        assert_eq!(*calls.borrow(), 0, "a removed cancel must never fire");
    }

    #[test]
    fn a_second_register_for_the_same_id_replaces_the_first() {
        // Documented as unreachable in practice (ids are never reused), but kept
        // total rather than panicking -- see `CancelMap::insert`'s own doc comment.
        let first_calls = Rc::new(RefCell::new(0));
        let second_calls = Rc::new(RefCell::new(0));
        let mut map = CancelMap::default();
        map.insert(1, {
            let c = Rc::clone(&first_calls);
            Box::new(move || *c.borrow_mut() += 1)
        });
        map.insert(1, {
            let c = Rc::clone(&second_calls);
            Box::new(move || *c.borrow_mut() += 1)
        });
        map.invoke(1);
        assert_eq!(*first_calls.borrow(), 0);
        assert_eq!(*second_calls.borrow(), 1);
    }

    // `ActivityRegistry` itself needs a live `MainWindow`, which the test suite cannot
    // start; `ActivityList` and `CancelMap` cover every decision it makes.
}
