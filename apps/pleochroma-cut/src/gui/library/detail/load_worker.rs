//! The bounded worker behind the local detail loads.
//!
//! One long-lived thread runs one request at a time, and at most ONE further request waits
//! for it: [`Worker::submit`] replaces whatever was waiting, because a request that a newer
//! click superseded before the thread reached it has nothing left to do. So a user who
//! clicks through a hundred rows leaves behind at most one running load and one pending
//! one, not a hundred threads each reading a design's attachments and solving it.
//!
//! The thread is started lazily on the first submit and is never joined (the history
//! thumbnail worker works the same way); dropping a [`Worker`] makes the thread exit after
//! the request it is on. If the OS refuses the thread, [`Worker::submit`] hands the request
//! back so the caller can run it inline, as the loader did before it had a worker.
//!
//! What a request does, and how it recognises itself as stale between its stages, is the
//! handler's business (`local_load`): this module only owns the queueing and the
//! [`LoadSequence`] numbering those stage checks read.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};
use tracing::{error, warn};

/// Numbers the local detail loads so a slow load that finishes after a newer click is
/// recognised as stale and dropped.
pub(super) struct LoadSequence(AtomicU64);

impl LoadSequence {
    pub(super) const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Starts a load: every earlier one is stale from now on.
    pub(super) fn begin(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    /// Whether `sequence` is the newest load.
    pub(super) fn is_current(&self, sequence: u64) -> bool {
        self.0.load(Ordering::Acquire) == sequence
    }
}

/// Whether the load numbered `sequence` should go on to its next stage (or, on the UI
/// thread, apply what a stage produced). A stale load returns without posting anything:
/// never as an error and never as a fallback.
pub(super) fn should_continue(loads: &LoadSequence, sequence: u64) -> bool {
    loads.is_current(sequence)
}

/// A request waiting for the thread.
struct Pending<R> {
    sequence: u64,
    request: R,
}

/// The one replaceable pending request, and whether the worker is closed.
struct State<R> {
    pending: Option<Pending<R>>,
    closed: bool,
}

impl<R> Default for State<R> {
    fn default() -> Self {
        Self {
            pending: None,
            closed: false,
        }
    }
}

impl<R> State<R> {
    /// Makes `request` the pending one. Returns the sequence of the request it replaced
    /// (which is dropped unrun), if there was one.
    fn replace(&mut self, sequence: u64, request: R) -> Option<u64> {
        self.pending
            .replace(Pending { sequence, request })
            .map(|replaced| replaced.sequence)
    }

    /// Takes the pending request.
    fn take(&mut self) -> Option<R> {
        self.pending.take().map(|pending| pending.request)
    }
}

/// The state both sides share, behind one mutex held only for bookkeeping, never while a
/// request runs.
struct Shared<R> {
    state: Mutex<State<R>>,
    /// Signalled when a request is queued or the worker is closed.
    wake: Condvar,
}

impl<R> Shared<R> {
    fn lock(&self) -> MutexGuard<'_, State<R>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Blocks until there is a request; `None` once the worker is closed.
    fn next_job(&self) -> Option<R> {
        let mut state = self
            .wake
            .wait_while(self.lock(), |state| {
                state.pending.is_none() && !state.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return None;
        }
        state.take()
    }
}

/// The handle the UI thread submits through.
pub(super) struct Worker<R: Send + 'static> {
    shared: Arc<Shared<R>>,
    handler: fn(R),
    /// Whether the thread has been started (guards the lazy start).
    thread_started: Mutex<bool>,
    /// The thread-refused warning is logged once, not per click.
    spawn_failure_logged: AtomicBool,
}

impl<R: Send + 'static> Worker<R> {
    /// A worker that runs `handler` on each request it takes. No thread starts until the
    /// first [`Self::submit`].
    pub(super) fn new(handler: fn(R)) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            }),
            handler,
            thread_started: Mutex::new(false),
            spawn_failure_logged: AtomicBool::new(false),
        }
    }

    /// Queues `request` (numbered `sequence`) as the one pending request, replacing the
    /// one that was waiting. Never blocks on the thread's work.
    ///
    /// # Errors
    ///
    /// Returns the request back when the OS refused to start the thread; the caller runs
    /// it inline.
    pub(super) fn submit(&self, sequence: u64, request: R) -> Result<(), R> {
        if !self.ensure_thread() {
            return Err(request);
        }
        self.shared.lock().replace(sequence, request);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Starts the thread if it is not running; whether it is.
    fn ensure_thread(&self) -> bool {
        let mut started = self
            .thread_started
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *started {
            return true;
        }
        // Held across the spawn so two clicks cannot start two threads.
        let shared = Arc::clone(&self.shared);
        let handler = self.handler;
        match thread::Builder::new()
            .name("library-detail".to_string())
            .spawn(move || run(&shared, handler))
        {
            Ok(_detached) => {
                *started = true;
                drop(started);
                true
            }
            Err(spawn_error) => {
                drop(started);
                // Left unstarted, so the next click tries again.
                if !self.spawn_failure_logged.swap(true, Ordering::Relaxed) {
                    warn!(
                        "could not start the library detail thread ({spawn_error}); \
                         detail loads run on the UI thread"
                    );
                }
                false
            }
        }
    }
}

impl<R: Send + 'static> Drop for Worker<R> {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_all();
    }
}

/// The thread's loop. A panic in `handler` is logged and the thread carries on, so one
/// bad design cannot silence every later click.
fn run<R>(shared: &Shared<R>, handler: fn(R)) {
    while let Some(request) = shared.next_job() {
        if catch_unwind(AssertUnwindSafe(|| handler(request))).is_err() {
            error!("a library detail load panicked; the worker thread carries on");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadSequence, State, Worker, should_continue};
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };

    #[test]
    fn only_the_newest_load_is_current() {
        let loads = LoadSequence::new();
        let first = loads.begin();
        assert!(loads.is_current(first));
        let second = loads.begin();
        assert_ne!(first, second);
        assert!(
            !loads.is_current(first),
            "a later click supersedes the first"
        );
        assert!(loads.is_current(second));
    }

    #[test]
    fn a_load_that_never_began_is_not_current() {
        let loads = LoadSequence::new();
        let _ = loads.begin();
        assert!(!loads.is_current(0));
    }

    #[test]
    fn a_stale_sequence_stops_at_the_stage_check() {
        let loads = LoadSequence::new();
        let first = loads.begin();
        assert!(should_continue(&loads, first));
        let second = loads.begin();
        assert!(!should_continue(&loads, first), "superseded: stop silently");
        assert!(should_continue(&loads, second));
    }

    #[test]
    fn submitting_twice_before_the_thread_takes_one_leaves_exactly_the_newest_pending() {
        let mut state = State::<&str>::default();
        assert_eq!(state.replace(1, "first"), None);
        assert_eq!(
            state.replace(2, "second"),
            Some(1),
            "the first request was replaced unrun"
        );
        assert_eq!(state.take(), Some("second"));
        assert_eq!(state.take(), None, "exactly one request was pending");
    }

    /// What `record` handed over, in the order the thread ran them.
    static RECORDED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    fn record(value: u32) {
        RECORDED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(value);
    }

    /// What `record_then_panic_on_zero` saw.
    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    /// Records the value, then panics if it is `0`.
    fn record_then_panic_on_zero(value: u32) {
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(value);
        assert_ne!(value, 0, "deliberate test panic");
    }

    fn wait_until(list: &Mutex<Vec<u32>>, value: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if list
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&value)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn the_thread_starts_on_the_first_submit_and_runs_the_request() {
        let worker = Worker::new(record);
        assert!(worker.submit(1, 11).is_ok());
        assert!(wait_until(&RECORDED, 11));
    }

    #[test]
    fn a_panicking_request_does_not_stop_the_thread() {
        let worker = Worker::new(record_then_panic_on_zero);
        assert!(worker.submit(1, 0).is_ok());
        assert!(wait_until(&SEEN, 0), "the panicking request ran");
        assert!(worker.submit(2, 22).is_ok());
        assert!(wait_until(&SEEN, 22), "the thread served the next request");
    }
}
