//! Starting `serve` in the background and stopping it again.
//!
//! # Stopping
//!
//! [`ShutdownHandle::shutdown`] (also [`super::ServeHandle::shutdown`]) does, in this order:
//!
//! 1. raises every listener's [`ListenerStop`] (viewer, worker port and both enrollment
//!    listeners) so no new connection is accepted and the ports are freed;
//! 2. removes every joined worker from the registry and closes every socket still in a
//!    handshake (worker side) or served (viewer side), which makes the threads blocked on
//!    them return;
//! 3. asks the job executor to stop (`worker` builds);
//! 4. waits, until the deadline, for the accept-loop threads, for the connection sets to
//!    empty and for the job executor.
//!
//! # What survives a timeout
//!
//! The call returns when the deadline passes and names what had not ended
//! ([`ShutdownReport::unfinished`]); it never blocks past it. Listeners were already told
//! to stop in step 1, so they end on their own. What can still run:
//!
//! - a connection thread in the middle of a render: its socket is closed, so it stops at
//!   its next write, but a trace that is already running finishes that batch first;
//! - the job executor, while it renders a job. The job is left `running` on disk and the
//!   next start puts it back in the queue with the restart note (see
//!   [`crate::jobs::JobService::shutdown`]); finished jobs and their pictures are
//!   untouched;
//! - the two enrollment accept loops, whose threads belong to the `enroll` module and are
//!   not waited for (they end as soon as their wake-up connection arrives).
//!
//! The process may exit while those run; nothing they hold needs saving.

use crate::{
    cli::ServeArgs,
    control::shutdown::{ConnectionSet, Latch, ListenerStop},
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use super::ServeHandle;

/// How long the headless command waits for a shutdown after Ctrl-C or SIGTERM.
pub const DEFAULT_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

/// How long a start that was cancelled (or whose receiver went away) waits for its own
/// shutdown.
const ABANDONED_START_DEADLINE: Duration = Duration::from_secs(5);

/// What [`ShutdownHandle::shutdown`] could not finish before its deadline. Empty when
/// everything ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Names of the parts that were still running at the deadline, for the log.
    pub unfinished: Vec<&'static str>,
}

impl ShutdownReport {
    /// Whether everything ended before the deadline.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.unfinished.is_empty()
    }
}

/// Everything shutdown has to stop, collected by [`super::start`] while it brings the
/// listeners up.
pub(super) struct Lifecycle {
    /// Every listener's stop: viewer, worker port, enrollment listeners.
    listeners: Vec<ListenerStop>,
    /// The accept-loop threads whose end is waited for, by name.
    loops: Vec<(&'static str, Latch)>,
    /// The sockets that are closed at shutdown, by name.
    connections: Vec<(&'static str, ConnectionSet)>,
    /// The joined workers, removed at shutdown.
    #[cfg(feature = "worker")]
    registry: Option<Arc<crate::coordinator::Registry>>,
    /// The job service, stopped at shutdown.
    #[cfg(feature = "worker")]
    jobs: Option<Arc<crate::jobs::JobService>>,
    /// The GPU shader warm-up, stopped at shutdown.
    #[cfg(feature = "worker")]
    warmup: Mutex<Option<super::warmup::Handle>>,
    /// The server-certificate address watch, stopped at shutdown (`None` without TLS).
    watch: Mutex<Option<crate::certs::AddressWatch>>,
    /// Set by the first shutdown call.
    started: AtomicBool,
    /// Raised when that call has finished (clean or at its deadline).
    finished: Latch,
    /// What the first call reported, for a second caller.
    report: Mutex<Option<ShutdownReport>>,
}

impl Lifecycle {
    /// An empty lifecycle.
    pub(super) fn new() -> Self {
        Self {
            listeners: Vec::new(),
            loops: Vec::new(),
            connections: Vec::new(),
            #[cfg(feature = "worker")]
            registry: None,
            #[cfg(feature = "worker")]
            jobs: None,
            #[cfg(feature = "worker")]
            warmup: Mutex::new(None),
            watch: Mutex::new(None),
            started: AtomicBool::new(false),
            finished: Latch::new(),
            report: Mutex::new(None),
        }
    }

    /// Records a listener that has an accept-loop thread signalling `exited`.
    pub(super) fn add_loop(&mut self, name: &'static str, stop: ListenerStop, exited: Latch) {
        self.listeners.push(stop);
        self.loops.push((name, exited));
    }

    /// Records a listener whose thread is not waited for (the enrollment listeners).
    pub(super) fn add_listener(&mut self, stop: ListenerStop) {
        self.listeners.push(stop);
    }

    /// Records the running certificate address watch.
    pub(super) fn set_watch(&self, watch: crate::certs::AddressWatch) {
        *self.watch.lock().unwrap_or_else(PoisonError::into_inner) = Some(watch);
    }

    /// Records a set of sockets to close at shutdown.
    pub(super) fn add_connections(&mut self, name: &'static str, set: ConnectionSet) {
        self.connections.push((name, set));
    }

    /// Records the joined-worker registry.
    #[cfg(feature = "worker")]
    pub(super) fn set_registry(&mut self, registry: Option<Arc<crate::coordinator::Registry>>) {
        self.registry = registry;
    }

    /// Records the job service.
    #[cfg(feature = "worker")]
    pub(super) fn set_jobs(&mut self, jobs: Option<Arc<crate::jobs::JobService>>) {
        self.jobs = jobs;
    }

    /// Records the running shader warm-up.
    #[cfg(feature = "worker")]
    pub(super) fn set_warmup(&self, handle: super::warmup::Handle) {
        *self.warmup.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
    }

    /// Whether a shutdown has begun.
    fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Removes every joined worker from the registry, which closes the idle ones.
    #[cfg(feature = "worker")]
    fn drop_joined_workers(&self) {
        if let Some(registry) = &self.registry {
            for (info, _idle) in registry.workers() {
                registry.remove(info.worker_id, "the coordinator is shutting down");
            }
        }
    }

    #[cfg(not(feature = "worker"))]
    #[expect(clippy::unused_self, reason = "the `worker` build has joined workers")]
    const fn drop_joined_workers(&self) {}

    /// Tells the job executor to stop after its current job.
    #[cfg(feature = "worker")]
    fn request_job_stop(&self) {
        if let Some(jobs) = &self.jobs {
            jobs.stop();
        }
    }

    #[cfg(not(feature = "worker"))]
    #[expect(clippy::unused_self, reason = "the `worker` build has a job service")]
    const fn request_job_stop(&self) {}

    /// Waits for the job executor until `end`; `true` when it ended (or there is none).
    #[cfg(feature = "worker")]
    fn job_executor_ended(&self, end: Instant) -> bool {
        self.jobs.as_ref().is_none_or(|jobs| jobs.shutdown(end))
    }

    #[cfg(not(feature = "worker"))]
    #[expect(clippy::unused_self, reason = "the `worker` build has a job service")]
    const fn job_executor_ended(&self, _end: Instant) -> bool {
        true
    }

    pub(super) fn shutdown(&self, deadline: Duration) -> ShutdownReport {
        let now = Instant::now();
        let end = now
            .checked_add(deadline)
            .unwrap_or_else(|| now + Duration::from_hours(365 * 24));
        if self.started.swap(true, Ordering::AcqRel) {
            // Another caller is shutting down (or already did): wait for it, no more.
            // The report below says whether the other call finished, so the flag is not needed.
            let _ = self.finished.wait_until(end);
            return self
                .report
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| ShutdownReport {
                    unfinished: vec!["shutdown (still running in another call)"],
                });
        }
        tracing::info!("pleochroma-worker serve: shutting down");

        for stop in &self.listeners {
            stop.stop();
        }
        let watch = self
            .watch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut watch) = watch {
            watch.stop();
        }
        self.drop_joined_workers();
        for (_, set) in &self.connections {
            set.close_all();
        }
        self.request_job_stop();
        #[cfg(feature = "worker")]
        let warmup = self
            .warmup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(warmup) = warmup {
            warmup.cancel();
        }

        let mut unfinished = Vec::new();
        for (name, exited) in &self.loops {
            if !exited.wait_until(end) {
                unfinished.push(*name);
            }
        }
        for (name, set) in &self.connections {
            if !set.wait_empty(end) {
                unfinished.push(*name);
            }
        }
        if !self.job_executor_ended(end) {
            unfinished.push("job executor");
        }

        let report = ShutdownReport { unfinished };
        if report.is_clean() {
            tracing::info!("pleochroma-worker serve: shut down");
        } else {
            tracing::warn!(
                "pleochroma-worker serve: shutdown deadline passed with {:?} still running",
                report.unfinished
            );
        }
        *self.report.lock().unwrap_or_else(PoisonError::into_inner) = Some(report.clone());
        self.finished.set();
        report
    }
}

/// A cheap, cloneable, thread-safe way to shut a running `serve` down, for a thread that
/// does not own the [`ServeHandle`] (the headless signal thread, a window's close button).
#[derive(Clone)]
pub struct ShutdownHandle {
    lifecycle: Arc<Lifecycle>,
}

impl ShutdownHandle {
    pub(super) const fn new(lifecycle: Arc<Lifecycle>) -> Self {
        Self { lifecycle }
    }

    /// Shuts `serve` down and waits up to `deadline` (see the module doc comment for the order and
    /// for what survives a timeout).
    ///
    /// Calling it again, from any clone, waits for the first call and returns its report.
    #[must_use]
    pub fn shutdown(&self, deadline: Duration) -> ShutdownReport {
        self.lifecycle.shutdown(deadline)
    }

    /// Whether a shutdown has begun.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.lifecycle.is_started()
    }

    /// Blocks until the shutdown that has begun is finished (it is bounded by that call's
    /// own deadline). Returns at once when none has begun.
    pub fn wait_finished(&self) {
        if self.is_shutting_down() {
            self.lifecycle.finished.wait();
        }
    }
}

/// A `serve` that is still starting, returned at once by [`start_background`].
///
/// Starting can take seconds (the GPU is acquired and a render split is calibrated before
/// the listeners open), so a window shows "Starting..." and polls this.
pub struct StartingServe {
    rx: Receiver<Result<ServeHandle, String>>,
    cancelled: Arc<AtomicBool>,
    taken: bool,
}

impl StartingServe {
    /// `None` while it is still starting; then, exactly once, the outcome. Never blocks.
    pub fn poll(&mut self) -> Option<Result<ServeHandle, String>> {
        if self.taken {
            return None;
        }
        match self.rx.try_recv() {
            Ok(result) => {
                self.taken = true;
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.taken = true;
                Some(Err("the start-up thread ended without a result".to_owned()))
            }
        }
    }

    /// Blocks until the start has finished.
    ///
    /// # Errors
    ///
    /// What [`super::start`] returns, or a message if the start was cancelled or its thread
    /// ended without a result.
    pub fn wait(self) -> Result<ServeHandle, String> {
        self.rx
            .recv()
            .unwrap_or_else(|_| Err("the start-up thread ended without a result".to_owned()))
    }

    /// Asks the start to be undone: when it has finished, whatever it started is shut down
    /// again and [`Self::poll`] reports an error. Does nothing after the outcome was taken.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Starts `serve` on a thread of its own and returns at once, so the caller can show "Starting..."
/// and poll [`StartingServe::poll`].
///
/// [`super::start`] does the same work on the calling thread.
#[must_use]
pub fn start_background(args: ServeArgs) -> StartingServe {
    spawn_start(args, || {})
}

/// [`start_background`] where `before` runs on the start-up thread first (tests hold the
/// start there).
pub(super) fn spawn_start(
    args: ServeArgs,
    before: impl FnOnce() + Send + 'static,
) -> StartingServe {
    let (tx, rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = Arc::clone(&cancelled);
    let thread_tx = tx.clone();
    let spawned = thread::Builder::new()
        .name("serve-start".to_owned())
        .spawn(move || {
            before();
            let result = match super::start(&args) {
                Ok(handle) if thread_cancelled.load(Ordering::Acquire) => {
                    let _ = handle.shutdown(ABANDONED_START_DEADLINE);
                    Err("starting was cancelled".to_owned())
                }
                other => other,
            };
            if let Err(unreceived) = thread_tx.send(result)
                && let Ok(handle) = unreceived.0
            {
                // Nobody is waiting any more: do not leave a coordinator running unseen.
                let _ = handle.shutdown(ABANDONED_START_DEADLINE);
            }
        });
    if let Err(error) = spawned {
        let _ = tx.send(Err(format!("could not start the start-up thread: {error}")));
    }
    StartingServe {
        rx,
        cancelled,
        taken: false,
    }
}

/// The headless run.
///
/// Starts `serve`, serves until `until` returns (Ctrl-C or SIGTERM in the command line program),
/// then shuts down within [`DEFAULT_SHUTDOWN_DEADLINE`] and returns, so the process exits with
/// status 0.
///
/// `until` runs on a thread of its own and blocks until a stop is wanted.
///
/// # Errors
///
/// What [`super::start`] returns, or a message if the viewer accept loop ended without a
/// shutdown having been asked for.
pub fn run_until(args: &ServeArgs, until: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let handle = start_background(args.clone()).wait()?;
    let stopper = handle.shutdown_handle();
    let signal_stopper = stopper.clone();
    thread::Builder::new()
        .name("serve-stop".to_owned())
        .spawn(move || {
            until();
            tracing::info!("pleochroma-worker serve: stop requested");
            let _ = signal_stopper.shutdown(DEFAULT_SHUTDOWN_DEADLINE);
        })
        .map_err(|e| format!("could not start the shutdown thread: {e}"))?;
    handle.wait();
    if !stopper.is_shutting_down() {
        return Err("the viewer accept loop ended unexpectedly".to_owned());
    }
    // The viewer loop is the first thing shutdown ends; the rest is still being stopped.
    stopper.wait_finished();
    Ok(())
}
