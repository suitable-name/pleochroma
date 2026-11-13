//! Off-UI-thread solving.
//!
//! One persistent worker thread owns a request queue with last-wins coalescing, real mid-solve
//! cancellation, throttled progress, and generation-tagged results that a caller can drop when
//! stale. It serves Deep Solve (`deep_solve::spawn_deep_solve`), the save and export solves
//! (`native_io::solve`) and the candidate-preview solves. `auto_solve::dispatch_background_solve`
//! (see [`SolveKind::Full`]) and the Optimize search (`optimize_solve::spawn_optimize_run`) keep
//! workers of their own. The UI thread never solves and never blocks.
//!
//! # Shape
//!
//! [`SolveService::new`] spawns ONE background thread for the service's lifetime. It loops:
//! take the next request from the [`Mailbox`], run it, report the result.
//! [`SolveService::submit`] never blocks. It overwrites the mailbox's single pending slot and
//! wakes the worker, so a burst of submits that land before the worker picks one up collapses
//! to a single solve of the LAST request. Requests that arrive mid-solve wait in the mailbox
//! for the next iteration.
//!
//! # Two independent kinds of "stale"
//!
//! 1. **Superseded by a later `submit`.** Every `submit` bumps a shared `latest_seq` counter
//!    and stamps its request with the new value. Before reporting, the worker compares its
//!    stamp with `latest_seq`. A mismatch sets [`SolveResult::superseded`]. The result is still
//!    delivered, so per-request bookkeeping keyed by [`SolveResult::generation`] (such as
//!    `native_io::solve`'s pending solves) is cleaned up and its waiting caller is told. A
//!    silent drop would strand a Save or Export with no file and no message.
//! 2. **The design changed for a reason that is not a new solve request** (an edit that landed
//!    while auto-solve is disabled, say). [`SolveResult::generation`] is the caller's own
//!    generation counter, stamped at submit time. This service does not know what is current.
//!    The caller compares it with its live counter at delivery and drops the result when they
//!    differ.
//!
//! # Cancellation
//!
//! [`SolveHandle::cancel`] sets the flag that `SolveControl::with_cancel` observes at every
//! cancel point `pleochroma` exposes (per sweep and per pipeline run). See the
//! `cancel_returns_quickly_on_the_largest_real_fixture` test. A request cancelled while still
//! queued is safe: the flag is set before the worker starts it, so it returns
//! [`SolveError::Cancelled`] almost at once.
//!
//! # Progress
//!
//! `on_progress` is throttled to about [`PROGRESS_THROTTLE`] (10 Hz) by [`ProgressThrottle`].
//! The solver can report many times per sweep, and forwarding each one across
//! `Weak::upgrade_in_event_loop` would cost the UI thread. The first report of each new
//! `(phase, sweep)` pair is always forwarded, so a progress bar never sits on a stale phase.

use super::auto_solve::solve_cancellably;
use pleochroma::geometry::{
    meet_solver::{
        SolveControl, SolveError, SolveProgress, SolvedTier, VerifiedSolveReport,
        solve_meet_points_verified_with,
    },
    stone_metrics::ExternalProportions,
};
use pleochroma_cut_core::{Design, DesignSolveError};
use slint::{ComponentHandle, Weak};
use std::{
    cell::Cell,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often a running solve's [`SolveProgress`] reports are actually forwarded to
/// the UI thread -- see the module doc comment, "Progress".
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);

/// What to solve, and how -- one worker request. See the module doc comment.
pub enum SolveKind {
    /// A full [`Design::solve_with`].
    ///
    /// Submitted by `native_io`'s save and export paths when no cached solve matches the
    /// current design (`native_io::solve::submit_full_solve`), and by this module's
    /// `cancel_returns_quickly_on_the_largest_real_fixture` test.
    ///
    /// `auto_solve::dispatch_background_solve` keeps its own worker instead. That worker also
    /// formats UI models (tier items, warnings, yield text), and `on_result` runs on the UI
    /// thread, so that formatting cannot move here.
    Full,
    /// `pleochroma::geometry::meet_solver::solve_meet_points_verified_with`'s externally
    /// verified repair search ("Deep Solve"). It passes no adjustable anchors, as `deep_solve`
    /// does.
    Verified {
        /// The printed proportions the repair search scores candidate mast
        /// configurations against.
        targets: ExternalProportions,
        /// Whether the worker should also compute this design's own plain solve (the
        /// "baseline" that the per-tier mast-delta table compares against) on this thread,
        /// before the verified search. `false` when the caller already holds a valid cached
        /// plain solve. It cannot be a separate request, because the mailbox is last-wins.
        compute_baseline: bool,
    },
    /// A candidate design solved for display in the viewport in place of the real
    /// one (Retarget's live overlay, Optimize's Preview toggle): the cancellable
    /// counterpart of [`Design::solve`], so it keeps that method's over-plane-cap
    /// fallback instead of surfacing it as an error the way [`Self::Full`] does.
    /// The request's own `generation` is the ghost's sequence number.
    ///
    /// Submit it on a service of its own: the mailbox is last-wins, so sharing one
    /// with a save or export would supersede that request's solve.
    GhostPreview,
}

/// [`SolveOutcome::Verified`]'s `Ok` payload: the verified repair search's own
/// result, plus the baseline plain solve computed alongside it when
/// [`SolveKind::Verified::compute_baseline`] asked for one.
pub struct VerifiedSolve {
    /// The verified repair search's own solved masts.
    pub solved: Vec<SolvedTier>,
    /// The aggregate verdict to display.
    pub report: VerifiedSolveReport,
    /// This design's own plain solve, computed on the SAME worker thread as
    /// `solved` above -- `None` when `compute_baseline` was `false` (the caller
    /// already had one), or when the plain solve itself failed (a design that
    /// does not even plain-solve). Either way, the caller omits its per-tier
    /// delta table rather than blocking on this.
    pub baseline: Option<Vec<SolvedTier>>,
}

/// One request for [`SolveService::submit`]. `design` is an `Arc` (not a plain
/// clone) so a caller that already snapshotted one for another purpose (a
/// solid-preview replan, say) can hand the SAME allocation to both without a
/// second `Design::clone`.
pub struct SolveRequest {
    pub design: Arc<Design>,
    /// The caller's own domain generation counter at submit time -- opaque to this
    /// module, carried through unchanged onto [`SolveResult::generation`] for the
    /// caller's own staleness check. See the module doc comment, "Two independent
    /// kinds of stale."
    pub generation: u64,
    pub kind: SolveKind,
}

/// What a finished (non-superseded) solve produced.
pub enum SolveOutcome {
    /// [`SolveKind::Full`]/[`SolveKind::GhostPreview`]'s result.
    /// Read by `native_io`'s save/export continuations
    /// (`native_io::resolve_solved_then`) and by the ghost preview's landing handler.
    Solved(Result<Vec<SolvedTier>, DesignSolveError>),
    /// [`SolveKind::Verified`]'s result.
    Verified(Result<VerifiedSolve, SolveError>),
}

/// A result from [`SolveService`]. Always delivered, even when a later submit superseded it.
pub struct SolveResult {
    /// Echoed from the [`SolveRequest`] this answers -- see
    /// [`SolveRequest::generation`]'s own doc comment.
    pub generation: u64,
    pub elapsed: Duration,
    pub outcome: SolveOutcome,
    /// `true` when a later `submit` moved the sequence past this request before it reported.
    /// `outcome` is the real computed result, but a caller must not apply or display it as
    /// current. It is delivered so that per-request bookkeeping keyed by [`Self::generation`]
    /// (`native_io::solve::PENDING_SOLVES`) is cleaned up and its waiting caller is told.
    pub superseded: bool,
}

/// A delivered, throttled progress tick -- generation-tagged the same way
/// [`SolveResult`] is, for a caller driving a per-generation progress banner.
#[derive(Debug, Clone, Copy)]
pub struct SolveProgressReport {
    pub generation: u64,
    pub progress: SolveProgress,
}

/// Returned by [`SolveService::submit`]. Cancelling stops EXACTLY this request --
/// see the module doc comment, "Cancellation".
pub struct SolveHandle {
    pub generation: u64,
    cancel: Arc<AtomicBool>,
}

impl SolveHandle {
    /// Requests cancellation -- see the module doc comment.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One queued request plus the bookkeeping the worker needs to decide whether its
/// eventual result still matters.
struct Queued {
    request: SolveRequest,
    seq: u64,
    cancel: Arc<AtomicBool>,
}

/// A single-slot, overwrite-on-put, block-until-present queue: the last-wins mailbox of the
/// module docs. It is not a channel, because a channel would need extra logic to drop all but
/// the newest entry, which `put` does directly.
///
/// It also carries the shutdown signal. [`Self::request_shutdown`] sets `shutdown` and wakes
/// the worker through the same `Condvar` as [`Self::put`].
struct Mailbox {
    slot: Mutex<Option<Queued>>,
    ready: Condvar,
    shutdown: AtomicBool,
}

impl Mailbox {
    const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Overwrites whatever was queued (if anything) and wakes one waiter.
    fn put(&self, item: Queued) {
        {
            let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            *slot = Some(item);
        }
        self.ready.notify_one();
    }

    /// Requests the worker's own exit -- see [`SolveService`]'s `Drop`
    /// impl, the one caller. Idempotent: a second call is a harmless repeat of
    /// the same store/wake.
    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.ready.notify_one();
    }

    /// Blocks until a request is available or shutdown was requested. `None` means shutdown,
    /// and the worker returns from [`worker_loop`]. A request already queued at shutdown is
    /// still returned first, so a racing `submit` is not dropped.
    fn take_blocking(&self) -> Option<Queued> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(item) = slot.take() {
                return Some(item);
            }
            if self.shutdown.load(Ordering::Relaxed) {
                return None;
            }
            slot = self
                .ready
                .wait(slot)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Non-blocking variant for tests: `None` if nothing is queued right now.
    #[cfg(test)]
    fn try_take(&self) -> Option<Queued> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.take()
    }
}

/// Per-request progress throttle (see the module docs, "Progress"). Built fresh for each
/// request, so a new solve always forwards its first report. It lives on the worker thread
/// only, so `Cell` suffices although [`SolveControl::reporting`] takes a `Fn`.
struct ProgressThrottle {
    last_forwarded: Cell<Option<Instant>>,
    last_phase_sweep: Cell<Option<(pleochroma::geometry::meet_solver::SolvePhase, u32)>>,
}

impl ProgressThrottle {
    const fn new() -> Self {
        Self {
            last_forwarded: Cell::new(None),
            last_phase_sweep: Cell::new(None),
        }
    }

    /// `true` the first time a given `(phase, sweep)` is seen, or once
    /// [`PROGRESS_THROTTLE`] has elapsed since the last forwarded report --
    /// otherwise `false` (this report is dropped, not delayed: the next one that
    /// clears the interval carries fresher numbers anyway).
    fn should_forward(&self, progress: SolveProgress) -> bool {
        let key = (progress.phase, progress.sweep);
        let new_phase_sweep = self.last_phase_sweep.get() != Some(key);
        let interval_elapsed = self
            .last_forwarded
            .get()
            .is_none_or(|t| t.elapsed() >= PROGRESS_THROTTLE);
        if !new_phase_sweep && !interval_elapsed {
            return false;
        }
        self.last_phase_sweep.set(Some(key));
        self.last_forwarded.set(Some(Instant::now()));
        true
    }
}

/// Runs one request to completion or cancellation, calling `on_progress` for each
/// [`SolveProgress`] (not throttled here; [`worker_loop`] applies [`ProgressThrottle`]). It
/// has no `Weak` or event-loop dependency, so a test can run it on a plain thread.
fn run_solve(
    request: &SolveRequest,
    cancel: &AtomicBool,
    on_progress: &dyn Fn(SolveProgress),
) -> SolveOutcome {
    let control = SolveControl::with_cancel(cancel).reporting(on_progress);
    match &request.kind {
        SolveKind::Full => SolveOutcome::Solved(request.design.solve_with(&control)),
        SolveKind::GhostPreview => SolveOutcome::Solved(solve_cancellably(&request.design, cancel)),
        SolveKind::Verified {
            targets,
            compute_baseline,
        } => {
            // The baseline runs first, on this thread, so the UI thread never solves it.
            // It shares `cancel` with the verified search below. A cancel during the
            // baseline stops it quickly, and the search then returns `SolveError::Cancelled`
            // at once.
            let baseline = (*compute_baseline)
                .then(|| solve_cancellably(&request.design, cancel).ok())
                .flatten();
            let gear_teeth_abs = request.design.meta.gear_teeth_abs();
            let tiers = request.design.meet_tier_inputs();
            SolveOutcome::Verified(
                solve_meet_points_verified_with(gear_teeth_abs, &tiers, targets, &[], &control)
                    .map(|(solved, report)| VerifiedSolve {
                        solved,
                        report,
                        baseline,
                    }),
            )
        }
    }
}

/// Owns the one persistent worker thread -- see the module doc comment.
pub struct SolveService {
    mailbox: Arc<Mailbox>,
    latest_seq: Arc<AtomicU64>,
}

impl SolveService {
    /// Spawns the worker thread. `on_progress`/`on_result` are invoked on the
    /// UI/event-loop thread via `Weak::upgrade_in_event_loop` -- both silently do
    /// nothing once `ui_weak` no longer upgrades (window closed while a solve was
    /// in flight), the same convention `deep_solve::spawn_deep_solve`/
    /// `optimize_solve::spawn_optimize_run` already follow.
    pub fn new<T, P, D>(ui_weak: Weak<T>, on_progress: P, on_result: D) -> Self
    where
        T: ComponentHandle + 'static,
        P: Fn(&T, SolveProgressReport) + Send + Clone + 'static,
        D: Fn(&T, SolveResult) + Send + Clone + 'static,
    {
        let mailbox = Arc::new(Mailbox::new());
        let latest_seq = Arc::new(AtomicU64::new(0));
        let worker_mailbox = Arc::clone(&mailbox);
        let worker_seq = Arc::clone(&latest_seq);
        thread::spawn(move || {
            worker_loop(
                &ui_weak,
                &worker_mailbox,
                &worker_seq,
                on_progress,
                on_result,
            );
        });
        Self {
            mailbox,
            latest_seq,
        }
    }

    /// Queues `request`, coalescing with (overwriting) anything not yet picked up
    /// by the worker -- see the module doc comment. Returns a handle that cancels
    /// EXACTLY this request.
    pub fn submit(&self, request: SolveRequest) -> SolveHandle {
        let seq = self.latest_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let generation = request.generation;
        self.mailbox.put(Queued {
            request,
            seq,
            cancel: Arc::clone(&cancel),
        });
        SolveHandle { generation, cancel }
    }
}

impl Drop for SolveService {
    /// Signals the worker thread to exit. Without this the worker would stay parked in
    /// [`Mailbox::take_blocking`] forever, because its own `Arc<Mailbox>` clone keeps the
    /// mailbox alive. The thread is not joined: `Drop` runs on the UI thread, and blocking
    /// there would defeat running solves off it.
    fn drop(&mut self) {
        self.mailbox.request_shutdown();
    }
}

/// The worker thread's whole life -- see the module doc comment for the two
/// staleness checks this performs (the `seq` one, inline below; the `generation`
/// one is the caller's own job once [`SolveResult`] is delivered).
fn worker_loop<T, P, D>(
    ui_weak: &Weak<T>,
    mailbox: &Mailbox,
    latest_seq: &AtomicU64,
    on_progress: P,
    on_result: D,
) where
    T: ComponentHandle + 'static,
    P: Fn(&T, SolveProgressReport) + Send + Clone + 'static,
    D: Fn(&T, SolveResult) + Send + Clone + 'static,
{
    loop {
        let Some(Queued {
            request,
            seq,
            cancel,
        }) = mailbox.take_blocking()
        else {
            // Shutdown requested by `SolveService::drop`. Nothing is reported: no caller
            // can still be waiting on `on_result`.
            return;
        };
        let generation = request.generation;
        let start = Instant::now();
        let throttle = ProgressThrottle::new();

        let outcome = run_solve(&request, &cancel, &|progress| {
            if latest_seq.load(Ordering::Relaxed) != seq || !throttle.should_forward(progress) {
                return;
            }
            let ui = ui_weak.clone();
            let on_progress = on_progress.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                on_progress(
                    &ui,
                    SolveProgressReport {
                        generation,
                        progress,
                    },
                );
            });
        });

        // Superseded by a later `submit` while this one was running -- still
        // delivered (see `SolveResult::superseded`'s own doc comment),
        // just tagged so the caller knows not to trust/apply `outcome`. The
        // pending request that superseded it is already sitting in `mailbox`,
        // ready for the next loop iteration either way.
        let superseded = latest_seq.load(Ordering::Relaxed) != seq;
        let result = SolveResult {
            generation,
            elapsed: start.elapsed(),
            outcome,
            superseded,
        };
        let ui = ui_weak.clone();
        let on_result = on_result.clone();
        let _ = ui.upgrade_in_event_loop(move |ui| {
            on_result(&ui, result);
        });
    }
}

#[cfg(test)]
mod tests;
