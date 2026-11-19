//! One thread at a time creates or tears down a GPU instance and device in this process.
//!
//! Concurrent creation and destruction stalled the Vulkan driver in user space on the test
//! machine: one thread waited in `pthread_join` for a thread that never ended, others queued
//! behind the driver's own locks, and the process stalled. Nothing in a single call was wrong;
//! the overlap was. So every acquisition holds the process-wide turn of this module, and a
//! [`GpuContext`](super::GpuContext) drops its handles inside the same turn once its last clone
//! has begun to drop.
//!
//! The teardown is complete only as far as the ownership allows: `wgpu` destroys a device when
//! its last handle or resource drops. The renderers declare their context as their last field,
//! so their pipelines and buffers are gone before it; a buffer that another owner keeps longer
//! ends the device outside the turn.
//!
//! Not compiled for wasm32: one thread, and the browser owns the device.

use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

/// A turn-taking lock whose guard is `Send`, so an async acquisition can hold it across its
/// awaits without making its future `!Send` (a `MutexGuard` would).
struct Gate {
    /// Whether a thread is inside a creation or a teardown.
    busy: Mutex<bool>,
    /// Signalled when `busy` turns false.
    freed: Condvar,
}

impl Gate {
    /// A free gate.
    #[allow(
        clippy::mutex_atomic,
        reason = "the flag is waited on with the condition variable"
    )]
    const fn new() -> Self {
        Self {
            busy: Mutex::new(false),
            freed: Condvar::new(),
        }
    }

    /// Waits until no other thread holds the turn, then takes it.
    fn enter(&'static self) -> Turn {
        let mut busy = self
            .freed
            .wait_while(
                self.busy.lock().unwrap_or_else(PoisonError::into_inner),
                |busy| *busy,
            )
            .unwrap_or_else(PoisonError::into_inner);
        *busy = true;
        drop(busy);
        Turn { gate: Some(self) }
    }

    /// Whether the turn stayed taken for the whole `wait`; never takes it.
    fn busy_for(&self, wait: Duration) -> bool {
        let (busy, result) = self
            .freed
            .wait_timeout_while(
                self.busy.lock().unwrap_or_else(PoisonError::into_inner),
                wait,
                |busy| *busy,
            )
            .unwrap_or_else(PoisonError::into_inner);
        drop(busy);
        let timed_out = result.timed_out();
        if !timed_out {
            // `Turn::drop` signals one waiter; this check may have taken that signal, so pass it
            // on to a thread that really waits for the turn.
            self.freed.notify_one();
        }
        timed_out
    }
}

/// The gate of the process.
static GATE: Gate = Gate::new();

/// The turn of one creation or teardown; dropping it lets the next thread in.
#[must_use = "the turn ends when the guard is dropped"]
pub(in crate::renderer::gpu) struct Turn {
    /// The gate to free; `None` for the pass-through turn of a test run with the gate off.
    gate: Option<&'static Gate>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(gate) = self.gate {
            *gate.busy.lock().unwrap_or_else(PoisonError::into_inner) = false;
            gate.freed.notify_one();
        }
    }
}

/// Takes the process-wide lifecycle turn, waiting for the thread that holds it.
///
/// A thread that holds the turn must not call this again (it would wait for itself): the
/// acquisitions release their turn before they build the context.
///
/// In a test build `PLEOCHROMA_GPU_TEST_GATE=0` makes this a pass-through, to reproduce the
/// stall.
pub(in crate::renderer::gpu) fn enter() -> Turn {
    #[cfg(test)]
    if super::test_support::gate_switched_off() {
        return Turn { gate: None };
    }
    GATE.enter()
}

/// Whether the process-wide turn stayed taken for the whole `wait`; never takes it.
pub(in crate::renderer::gpu) fn busy_for(wait: Duration) -> bool {
    GATE.busy_for(wait)
}

/// What the clones of one context share for its teardown.
struct Family {
    /// The clones whose [`TeardownBegin`] has not begun to drop.
    begins: AtomicUsize,
    /// The clones whose [`TeardownEnd`] has not dropped.
    ends: AtomicUsize,
    /// The turn, from the moment the last clone begins to drop until every clone has dropped.
    closing: Mutex<Option<Turn>>,
    /// The gate of a unit test; `None` is the process gate through [`enter`].
    gate: Option<&'static Gate>,
    /// In a test build, the watch of the teardown span: the watchdog reports a teardown that
    /// runs past its bound, with the thread that took the turn.
    #[cfg(test)]
    teardown_watch: Mutex<Option<super::test_support::Watch>>,
}

impl Family {
    /// The turn of this family's gate.
    fn enter(&self) -> Turn {
        self.gate.map_or_else(enter, Gate::enter)
    }
}

/// The FIRST field of a `GpuContext`, so it drops before the `wgpu` handles.
///
/// When the last clone of the context begins to drop, this takes the lifecycle turn; the
/// [`TeardownEnd`] that drops last gives it back.
pub(in crate::renderer::gpu) struct TeardownBegin(Arc<Family>);

/// The LAST field of a `GpuContext`, so it drops after the `wgpu` handles.
///
/// The turn its family took is given back when the end of every clone has dropped: a clone that
/// began to drop earlier on another thread may still hold its handles.
pub(in crate::renderer::gpu) struct TeardownEnd(Arc<Family>);

/// The two teardown fields of a new context.
pub(in crate::renderer::gpu) fn teardown_pair() -> (TeardownBegin, TeardownEnd) {
    pair_on(None)
}

/// [`teardown_pair`] on `gate` (`None`: the process gate).
fn pair_on(gate: Option<&'static Gate>) -> (TeardownBegin, TeardownEnd) {
    let family = Arc::new(Family {
        begins: AtomicUsize::new(1),
        ends: AtomicUsize::new(1),
        closing: Mutex::new(None),
        gate,
        #[cfg(test)]
        teardown_watch: Mutex::new(None),
    });
    (TeardownBegin(Arc::clone(&family)), TeardownEnd(family))
}

impl Clone for TeardownBegin {
    fn clone(&self) -> Self {
        // A clone is made from a live context, so the count is at least one here.
        self.0.begins.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(&self.0))
    }
}

impl Drop for TeardownBegin {
    fn drop(&mut self) {
        if self.0.begins.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let turn = self.0.enter();
        #[cfg(test)]
        {
            *self
                .0
                .teardown_watch
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(super::test_support::watch(
                "tearing down a GPU device inside the lifecycle turn",
                super::test_support::ACQUIRE_BOUND,
            ));
        }
        *self
            .0
            .closing
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(turn);
    }
}

impl Clone for TeardownEnd {
    fn clone(&self) -> Self {
        // A clone is made from a live context, so the count is at least one here.
        self.0.ends.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(&self.0))
    }
}

impl Drop for TeardownEnd {
    fn drop(&mut self) {
        if self.0.ends.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        // The last end: every clone's handles are gone. The begin of the clone this end belongs
        // to dropped before it, so the turn of the last begin is stored by now.
        #[cfg(test)]
        drop(
            self.0
                .teardown_watch
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let turn = self
            .0
            .closing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(turn);
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Gate, pair_on};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A gate of its own, so the tests do not meet the GPU tests of the same process.
    fn leaked_gate() -> &'static Gate {
        Box::leak(Box::new(Gate::new()))
    }

    fn busy(gate: &Gate) -> bool {
        *gate.busy.lock().expect("not poisoned")
    }

    #[test]
    fn one_thread_at_a_time_holds_the_turn() {
        let gate = leaked_gate();
        let inside = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        let turn = gate.enter();
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        std::thread::yield_now();
                        inside.fetch_sub(1, Ordering::SeqCst);
                        drop(turn);
                    }
                });
            }
        });
        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert!(!busy(gate));
    }

    #[test]
    fn a_gate_is_busy_for_the_wait_only_while_a_turn_is_taken() {
        let gate = leaked_gate();
        assert!(!gate.busy_for(Duration::from_millis(20)));
        assert!(!busy(gate), "the check took the turn");
        let turn = gate.enter();
        assert!(gate.busy_for(Duration::from_millis(20)));
        drop(turn);
        assert!(!gate.busy_for(Duration::from_millis(20)));
    }

    #[test]
    fn only_the_last_clone_takes_the_turn_and_holds_it_between_its_two_ends() {
        let gate = leaked_gate();
        let (begin, end) = pair_on(Some(gate));
        let (begin_clone, end_clone) = (begin.clone(), end.clone());
        drop(begin_clone);
        assert!(!busy(gate), "a clone that is not the last took the turn");
        drop(end_clone);
        drop(begin);
        assert!(busy(gate), "the last clone did not take the turn");
        drop(end);
        assert!(!busy(gate), "the turn was not given back");
    }

    #[test]
    fn the_turn_is_held_until_the_end_of_every_clone_has_dropped() {
        let gate = leaked_gate();
        let (begin, end) = pair_on(Some(gate));
        let (begin_clone, end_clone) = (begin.clone(), end.clone());
        // An earlier clone has begun to drop on its own thread and still holds its handles.
        drop(begin_clone);
        // The last clone drops completely.
        drop(begin);
        drop(end);
        assert!(
            busy(gate),
            "the turn ended while an earlier clone still held its handles"
        );
        std::thread::scope(|scope| {
            scope.spawn(move || drop(end_clone));
        });
        assert!(!busy(gate), "the turn was not given back");
    }
}
