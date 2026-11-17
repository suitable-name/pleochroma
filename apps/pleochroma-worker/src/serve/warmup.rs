//! The GPU shader warm-up of a render computer (`serve --render` and `join`), built in the
//! background as soon as the computer starts.
//!
//! The first Ray-Traced and Path-Traced picture of a process waits for its kernels to compile:
//! one Ray-Traced kernel, and a set of Path-Traced kernels per material type. This job compiles
//! them up front, like Pleochroma Cut's shader cache job: Ray-Traced first, because its one
//! kernel is quick, then every material type of the catalogue (`warm_plan::catalogue_steps`)
//! plain and on the tray. The Path-Traced steps run through `warm_plan::warm_in_parallel` on
//! the computer's own [`GpuBackend`] (the one its requests render on) and, when it keeps a disk
//! cache, on helper devices that compile shares of the steps in parallel into that cache. The
//! lane takes one FIFO turn per step, so a real request waits for at most one step.
//!
//! # Reading the state
//!
//! [`state`] and [`preparing`] never wait for a render: the state is a small value behind a
//! mutex that is only held for a copy, never across GPU work. The coordinator window and the
//! remote administration read it through `CoordinatorStatus::shader_cache`; the stream emitter
//! reads [`preparing`] to send `PREPARING` instead of `PROGRESS` while a request has traced
//! nothing yet.
//!
//! # Joined computers
//!
//! A joined computer that warms up sends `PREPARING` to the coordinator like any viewer's
//! server would. The coordinator notes it per computer ([`note_remote`], [`clear_remote`]) and
//! [`preparing`] adds those to its own progress, so a viewer behind the coordinator hears one
//! combined `PREPARING` and never a `PROGRESS` with no samples in between.

use pleochroma::renderer::gpu_backend::{
    GpuAccumulate, GpuBackend, GpuWarmFamilies,
    warm_plan::{StepDone, catalogue_steps, compile_threads, warm_in_parallel},
};
use pleochroma_net::admin::status::{ShaderCacheState, ShaderCacheStatus};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Instant,
};

/// Every kernel family this build has: the production kernels, the path cache's and, with
/// adaptive sampling built in, the live ones.
const ALL_FAMILIES: GpuWarmFamilies = GpuWarmFamilies {
    live: cfg!(feature = "adaptive-sampling"),
    path_cache: true,
};

/// What one step of the job did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// Built, or declined by the device (a tray it cannot bind): the step is done.
    Built,
    /// A cancel arrived: the step is unbuilt.
    Cancelled,
    /// The device was lost: nothing more can be built.
    Lost,
}

/// One step's report to [`Progress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StepReport {
    outcome: Outcome,
    /// The step compiled a kernel from source instead of loading it from the disk cache.
    fresh: bool,
}

/// How a run of steps ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Ready,
    Stopped,
    Unavailable,
}

/// The process-wide warm-up state.
struct Tracker {
    status: Mutex<ShaderCacheStatus>,
}

impl Tracker {
    const fn new() -> Self {
        Self {
            status: Mutex::new(ShaderCacheStatus {
                state: ShaderCacheState::Idle,
                done: 0,
                total: 0,
                fresh: false,
            }),
        }
    }

    fn get(&self) -> ShaderCacheStatus {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut ShaderCacheStatus)) {
        change(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// Takes the job: `Running` with no count yet. `false` while another job runs.
    fn claim(&self) -> bool {
        let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        if status.state == ShaderCacheState::Running {
            return false;
        }
        *status = ShaderCacheStatus {
            state: ShaderCacheState::Running,
            done: 0,
            total: 0,
            fresh: false,
        };
        true
    }

    /// The step count is known; `fresh` is set at once for a device that keeps no cache.
    fn begin(&self, total: u32, fresh: bool) {
        self.update(|status| {
            status.total = total;
            status.fresh |= fresh;
        });
    }

    /// One more step is done. Steps finish on several threads and in any order, so the count
    /// is raised under the lock instead of being set.
    fn bump(&self) {
        self.update(|status| status.done = status.done.saturating_add(1));
    }

    fn mark_fresh(&self) {
        self.update(|status| status.fresh = true);
    }

    fn finish(&self, ending: Ending) {
        self.update(|status| {
            status.state = match ending {
                Ending::Ready => ShaderCacheState::Ready,
                Ending::Stopped => ShaderCacheState::Stopped,
                Ending::Unavailable => ShaderCacheState::Unavailable,
            };
            if ending == Ending::Ready {
                status.done = status.total;
            }
        });
    }
}

/// Ends a job that unwinds as `Stopped`, so the state never stays `Running`.
struct EndGuard<'a>(&'a Tracker);

impl Drop for EndGuard<'_> {
    fn drop(&mut self) {
        self.0.update(|status| {
            if status.state == ShaderCacheState::Running {
                status.state = ShaderCacheState::Stopped;
            }
        });
    }
}

static TRACKER: Tracker = Tracker::new();

/// The joined computers' `PREPARING` counts (`done`, `total`) by registry id, while they have
/// traced nothing.
static REMOTE: Mutex<BTreeMap<u32, (u32, u32)>> = Mutex::new(BTreeMap::new());

/// This computer's warm-up state, for `CoordinatorStatus::shader_cache`. Never waits for a
/// render.
#[must_use]
pub fn state() -> ShaderCacheStatus {
    TRACKER.get()
}

/// `Some((done, total))` while a request that has traced nothing should be told the graphics
/// card is being prepared: this computer's warm-up runs, or a joined computer reported
/// `PREPARING`. Several are added up (`total == 0`: unknown). Never waits for a render.
#[must_use]
pub fn preparing() -> Option<(u32, u32)> {
    let remote = REMOTE.lock().unwrap_or_else(PoisonError::into_inner);
    combine(TRACKER.get(), remote.values().copied())
}

/// The sum of this computer's running warm-up and the joined computers' counts.
fn combine(own: ShaderCacheStatus, remote: impl Iterator<Item = (u32, u32)>) -> Option<(u32, u32)> {
    let mut found = false;
    let (mut done, mut total) = (0_u32, 0_u32);
    if own.state == ShaderCacheState::Running {
        found = true;
        (done, total) = (own.done, own.total);
    }
    for (d, t) in remote {
        found = true;
        done = done.saturating_add(d);
        total = total.saturating_add(t);
    }
    found.then_some((done, total))
}

/// Records that joined computer `worker_id` reported `PREPARING` for its chunk.
pub fn note_remote(worker_id: u32, done: u32, total: u32) {
    REMOTE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(worker_id, (done, total));
}

/// Forgets joined computer `worker_id`'s `PREPARING` (its samples arrived, or its chunk ended).
pub fn clear_remote(worker_id: u32) {
    REMOTE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&worker_id);
}

/// A running warm-up, stopped through it.
#[derive(Clone, Debug)]
pub struct Handle {
    cancel: Arc<AtomicBool>,
}

impl Handle {
    /// Stops the job after its current step; a compile already in the driver runs to its end.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// The progress of one run: steps are reported from several threads, in any order.
struct Progress<'a> {
    tracker: &'a Tracker,
    /// Steps that compiled a kernel from source.
    fresh_steps: AtomicU32,
    /// The first reason the run cannot go on, if any.
    halt: Mutex<Option<Ending>>,
}

impl<'a> Progress<'a> {
    const fn new(tracker: &'a Tracker) -> Self {
        Self {
            tracker,
            fresh_steps: AtomicU32::new(0),
            halt: Mutex::new(None),
        }
    }

    /// Records one finished step in the tracker.
    fn record(&self, report: StepReport) {
        if report.fresh {
            self.tracker.mark_fresh();
            self.fresh_steps.fetch_add(1, Ordering::Relaxed);
        }
        match report.outcome {
            Outcome::Built => self.tracker.bump(),
            Outcome::Cancelled => self.stop(Ending::Stopped),
            Outcome::Lost => self.stop(Ending::Unavailable),
        }
    }

    fn stop(&self, ending: Ending) {
        self.halt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(ending);
    }

    /// Whether a step ended the run.
    fn halted(&self) -> bool {
        self.halt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn fresh_steps(&self) -> u32 {
        self.fresh_steps.load(Ordering::Relaxed)
    }

    /// Ends the run: the first halting step wins, then a cancel that left steps undone, else
    /// `Ready` (a cancel after the last step changes nothing).
    fn conclude(&self, cancel: &AtomicBool) -> Ending {
        let halted = *self.halt.lock().unwrap_or_else(PoisonError::into_inner);
        let status = self.tracker.get();
        let all_done = status.total > 0 && status.done >= status.total;
        let ending = halted.unwrap_or_else(|| {
            if cancel.load(Ordering::Acquire) && !all_done {
                Ending::Stopped
            } else {
                Ending::Ready
            }
        });
        self.tracker.finish(ending);
        ending
    }
}

/// How a finished step counts: a cancel stops the run, a lost lane device ends it.
const fn classify(cancelled: bool, lane_lost: bool) -> Outcome {
    if cancelled {
        Outcome::Cancelled
    } else if lane_lost {
        Outcome::Lost
    } else {
        Outcome::Built
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Starts the warm-up of `gpu` in the background. `who` names the caller in the log (`serve` or
/// `join`). `None` when this build has no graphics card kernels, `gpu` has no usable device, a
/// warm-up already runs, or the thread could not start.
#[must_use]
pub fn start(gpu: Arc<GpuBackend>, who: &'static str) -> Option<Handle> {
    if !cfg!(feature = "gpu") || gpu.adapter_label().is_none() {
        return None;
    }
    if !TRACKER.claim() {
        return None;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let for_thread = Arc::clone(&cancel);
    let spawned = std::thread::Builder::new()
        .name("pleochroma-shader-warmup".to_owned())
        .spawn(move || run(&gpu, &for_thread, who));
    if let Err(error) = spawned {
        tracing::warn!(
            %error,
            "pleochroma-worker {who}: could not start the shader warm-up; the kernels build on first use"
        );
        TRACKER.update(|status| status.state = ShaderCacheState::Idle);
        return None;
    }
    Some(Handle { cancel })
}

/// The thread body: the Ray-Traced kernel, then every Path-Traced material type, plain and on
/// the tray, on as many compile threads as the machine allows.
fn run(gpu: &GpuBackend, cancel: &AtomicBool, who: &str) {
    let _end = EndGuard(&TRACKER);
    let started = Instant::now();
    let steps = catalogue_steps(None);
    let total = count(1 + steps.len());
    let persistent = gpu.persists_pipelines();
    // A device that keeps no cache cannot share work with helpers: one thread.
    let threads = if persistent { compile_threads() } else { 1 };
    // A device that keeps no cache compiles every kernel at every start: always fresh.
    TRACKER.begin(total, !persistent);
    tracing::info!(
        "pleochroma-worker {who}: shader warm-up started ({total} steps, disk cache {}, {threads} \
         compile thread{})",
        if persistent { "kept" } else { "not kept" },
        if threads == 1 { "" } else { "s" }
    );
    let progress = Progress::new(&TRACKER);
    // Ray-Traced first: its one kernel is quick.
    let compiled = GpuBackend::fresh_pipeline_compiles();
    let outcome = ray_step(cancel);
    progress.record(StepReport {
        outcome,
        fresh: !persistent || GpuBackend::fresh_pipeline_compiles() > compiled,
    });
    if !progress.halted() {
        warm_in_parallel(
            gpu,
            &steps,
            ALL_FAMILIES,
            threads,
            cancel,
            |done: StepDone| {
                let outcome = classify(
                    matches!(done.outcome, GpuAccumulate::Cancelled),
                    done.on_lane && gpu.is_lost(),
                );
                tracing::debug!(
                    index = done.index,
                    fresh = done.fresh,
                    on_lane = done.on_lane,
                    ?outcome,
                    elapsed_ms = started.elapsed().as_millis(),
                    "shader warm-up step"
                );
                progress.record(StepReport {
                    outcome,
                    fresh: done.fresh,
                });
            },
        );
    }
    let ending = progress.conclude(cancel);
    let status = TRACKER.get();
    let fresh_steps = progress.fresh_steps();
    // Per step, so one fresh kernel among many cached ones is told apart from a cold start.
    tracing::info!(
        "pleochroma-worker {who}: shader warm-up ended {ending:?} ({} of {} steps: {fresh_steps} \
         compiled, {} loaded from the cache) in {:.1} s",
        status.done,
        status.total,
        status.done.saturating_sub(fresh_steps),
        started.elapsed().as_secs_f32()
    );
}

/// The Ray-Traced backend's kernel: the lane's static backend acquires (and compiles) on first
/// use.
fn ray_step(cancel: &AtomicBool) -> Outcome {
    if cancel.load(Ordering::Acquire) {
        return Outcome::Cancelled;
    }
    // Whether the device exists does not matter here: without one the lane traces on the CPU.
    let _ = crate::render_core::whitted::warm_up();
    Outcome::Built
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(fresh: bool) -> StepReport {
        StepReport {
            outcome: Outcome::Built,
            fresh,
        }
    }

    fn report(outcome: Outcome) -> StepReport {
        StepReport {
            outcome,
            fresh: false,
        }
    }

    #[test]
    fn the_job_goes_idle_running_ready() {
        let tracker = Tracker::new();
        assert_eq!(tracker.get().state, ShaderCacheState::Idle);
        assert!(tracker.claim());
        assert!(!tracker.claim(), "one job at a time");
        tracker.begin(3, false);
        assert_eq!(tracker.get().state, ShaderCacheState::Running);
        let cancel = AtomicBool::new(false);
        let progress = Progress::new(&tracker);
        let mut seen = Vec::new();
        for index in 0..3 {
            seen.push((index, tracker.get().done));
            progress.record(built(index == 1));
        }
        assert_eq!(progress.conclude(&cancel), Ending::Ready);
        assert_eq!(seen, vec![(0, 0), (1, 1), (2, 2)]);
        assert_eq!(progress.fresh_steps(), 1);
        let status = tracker.get();
        assert_eq!(
            (status.state, status.done, status.total, status.fresh),
            (ShaderCacheState::Ready, 3, 3, true)
        );
        assert!(tracker.claim(), "a finished job can be started again");
        assert!(!tracker.get().fresh, "a new job starts not fresh");
    }

    #[test]
    fn a_cancel_stops_the_job() {
        let tracker = Tracker::new();
        assert!(tracker.claim());
        tracker.begin(5, false);
        let cancel = AtomicBool::new(false);
        let progress = Progress::new(&tracker);
        progress.record(built(false));
        progress.record(built(false));
        cancel.store(true, Ordering::Release);
        assert_eq!(progress.conclude(&cancel), Ending::Stopped);
        let status = tracker.get();
        assert_eq!(status.state, ShaderCacheState::Stopped);
        assert_eq!((status.done, status.total), (2, 5));
    }

    #[test]
    fn a_cancel_after_the_last_step_still_ends_ready() {
        let tracker = Tracker::new();
        assert!(tracker.claim());
        tracker.begin(2, false);
        let cancel = AtomicBool::new(false);
        let progress = Progress::new(&tracker);
        progress.record(built(false));
        progress.record(built(true));
        cancel.store(true, Ordering::Release);
        assert_eq!(progress.conclude(&cancel), Ending::Ready);
        assert_eq!(tracker.get().state, ShaderCacheState::Ready);
    }

    #[test]
    fn a_cancelled_step_is_not_counted_and_a_lost_device_is_unavailable() {
        let tracker = Tracker::new();
        assert!(tracker.claim());
        tracker.begin(4, false);
        let cancel = AtomicBool::new(false);
        let progress = Progress::new(&tracker);
        progress.record(report(Outcome::Built));
        progress.record(report(Outcome::Built));
        assert!(!progress.halted());
        progress.record(report(Outcome::Cancelled));
        assert!(progress.halted());
        assert_eq!(progress.conclude(&cancel), Ending::Stopped);
        assert_eq!(tracker.get().done, 2);

        assert!(tracker.claim());
        tracker.begin(4, false);
        let progress = Progress::new(&tracker);
        progress.record(report(Outcome::Lost));
        progress.record(report(Outcome::Cancelled));
        assert_eq!(
            progress.conclude(&cancel),
            Ending::Unavailable,
            "the first reason wins"
        );
        assert_eq!(tracker.get().state, ShaderCacheState::Unavailable);
    }

    #[test]
    fn steps_reported_from_several_threads_are_all_counted() {
        let tracker = Tracker::new();
        assert!(tracker.claim());
        tracker.begin(100, false);
        let progress = Progress::new(&tracker);
        std::thread::scope(|scope| {
            for thread in 0..4_u32 {
                let progress = &progress;
                scope.spawn(move || {
                    for step in 0..25_u32 {
                        progress.record(built(step % 5 == thread));
                    }
                });
            }
        });
        let cancel = AtomicBool::new(false);
        assert_eq!(progress.conclude(&cancel), Ending::Ready);
        let status = tracker.get();
        assert_eq!((status.done, status.total), (100, 100));
        assert!(status.fresh);
        assert_eq!(progress.fresh_steps(), 20);
    }

    #[test]
    fn a_finished_step_is_classified() {
        assert_eq!(classify(false, false), Outcome::Built);
        assert_eq!(classify(true, false), Outcome::Cancelled);
        assert_eq!(classify(false, true), Outcome::Lost);
        assert_eq!(classify(true, true), Outcome::Cancelled);
    }

    #[test]
    fn a_job_that_unwinds_ends_stopped() {
        let tracker = Tracker::new();
        assert!(tracker.claim());
        drop(EndGuard(&tracker));
        assert_eq!(tracker.get().state, ShaderCacheState::Stopped);
        // A finished job keeps its state.
        tracker.finish(Ending::Ready);
        drop(EndGuard(&tracker));
        assert_eq!(tracker.get().state, ShaderCacheState::Ready);
    }

    #[test]
    fn preparing_adds_the_own_warm_up_and_the_joined_computers() {
        let running = ShaderCacheStatus {
            state: ShaderCacheState::Running,
            done: 3,
            total: 20,
            fresh: true,
        };
        let idle = ShaderCacheStatus::default();
        assert_eq!(combine(idle, std::iter::empty()), None);
        assert_eq!(combine(running, std::iter::empty()), Some((3, 20)));
        assert_eq!(combine(idle, [(1, 10), (2, 5)].into_iter()), Some((3, 15)));
        assert_eq!(
            combine(running, [(1, 10)].into_iter()),
            Some((4, 30)),
            "one combined count, not two events"
        );
        let ready = ShaderCacheStatus {
            state: ShaderCacheState::Ready,
            ..running
        };
        assert_eq!(combine(ready, std::iter::empty()), None);
    }
}
