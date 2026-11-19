//! The render worker pool: the state the worker threads share, the epoch they trace, and
//! the frames they publish.
//!
//! # Epochs
//!
//! One epoch is one scene at one size. Its frame is cut into `P` interleaved partitions
//! (`P` is the pool's thread cap, fixed for the pool's life); every thread takes the next
//! chunk from the epoch's [`ChunkPlanner`], traces it against the epoch's shared
//! [`OwnedScene`] with an abort check, and merges the sums into the epoch's
//! [`Accumulator`]. A pass (all partitions for one sample range) is merged only when it is
//! complete, so every pixel shows the same sample count. Starting a new epoch changes the
//! atomic epoch id: running chunks notice within a few milliseconds, give their work back
//! and their results, if any, are dropped as stale.
//!
//! # Frames
//!
//! After every merged pass the committing thread tone-maps the running mean into a frame
//! slot (newest wins, never an older pass over a newer one). When the target is reached
//! and denoise is on, the denoised picture follows. The UI polls the slot.
//!
//! # Locking
//!
//! `Shared::inner` guards the epoch and the controls and is never held while tracing,
//! tone-mapping or denoising. The frame slot and the denoiser have their own locks and
//! are never taken while `inner` is held.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use glam::Vec3;
use pleochroma::{diag, renderer::tonemap::tonemap_to_rgba_with_threads};
use pleochroma_web_core::{
    display::{DenoiseFrame, Denoiser},
    render::{Accumulator, ChunkAssignment, ChunkOutcome, ChunkPlanner, ChunkRange},
    scene::OwnedScene,
};

use super::{
    PauseReason, RenderFrame, RenderStatus,
    backend::{BackendManager, TraceBackend},
};
use crate::{settings::RenderBackend, viewport::render_view::PreparationTime};

/// How long an idle worker sleeps before it looks for work again (a wake-up is also sent
/// whenever work may have appeared).
const IDLE_WAIT: Duration = Duration::from_millis(500);

/// The smoothing of the measured per-thread trace rate.
const RATE_SMOOTHING: f64 = 0.5;

/// Reported when a render thread panicked or the accumulator refused a chunk. The next start
/// begins a fresh epoch.
pub(super) const RUN_FAILED: &str = "The render stopped unexpectedly. It starts again by itself.";

/// Locks `mutex`, recovering the data of a poisoned lock: a panicking tracer must not take
/// the whole pool down with it.
fn relock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the current epoch stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Nothing runs.
    Idle,
    /// The scene is being built or traced.
    Active,
    /// The target is reached.
    Done,
    /// The epoch ended with this message.
    Failed(String),
}

/// What limits the workers, set by the user, the lifecycle and the governor.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches set by the user, the lifecycle and the governor"
)]
pub(super) struct Control {
    /// The threads the pool runs (and the partitions of a frame).
    pub(super) max_workers: usize,
    /// How many of them may trace now (the governor's decision).
    pub(super) workers_allowed: usize,
    /// The user paused tracing.
    pub(super) user_paused: bool,
    /// The app is in the background.
    pub(super) background: bool,
    /// The device is too hot to trace.
    pub(super) hot: bool,
    /// Whether the settled picture is denoised.
    pub(super) denoise: bool,
}

impl Control {
    /// Why tracing is paused, if it is (user first: it is the one the user can undo).
    pub(super) const fn paused_reason(&self) -> Option<PauseReason> {
        if self.user_paused {
            Some(PauseReason::User)
        } else if self.background {
            Some(PauseReason::Background)
        } else if self.hot {
            Some(PauseReason::Hot)
        } else {
            None
        }
    }
}

/// One scene being traced.
struct Epoch {
    backend: TraceBackend,
    cancel: Arc<AtomicBool>,
    /// Raised (never cleared, never reused) when the epoch ends: replaced, stopped or the
    /// pool closed. Unlike `cancel` it does NOT follow pause/resume. A [`Follow`] carries
    /// a clone, so a denoise of this epoch's picture stops at its next cancel point.
    obsolete: Arc<AtomicBool>,
    id: u64,
    scene: Arc<OwnedScene>,
    planner: ChunkPlanner,
    acc: Accumulator,
    /// The partition the next chunk search starts at, so the partitions are served evenly.
    cursor: u32,
    /// Per partition: a chunk of it is being traced. One at a time keeps every hand-back
    /// ([`ChunkPlanner::unassign`]) valid.
    busy: Vec<bool>,
    target: u32,
    finished: bool,
}

/// A chunk handed to a thread.
pub(super) struct Job {
    backend: TraceBackend,
    cancel: Arc<AtomicBool>,
    epoch_id: u64,
    assignment: ChunkAssignment,
    scene: Arc<OwnedScene>,
}

/// A copy of the merged frame, taken under the lock and shown outside it.
pub(super) struct Follow {
    epoch_id: u64,
    /// The epoch's [`Epoch::obsolete`] flag.
    obsolete: Arc<AtomicBool>,
    width: u32,
    height: u32,
    samples: u32,
    target: u32,
    sum: Vec<Vec3>,
    scene: Arc<OwnedScene>,
    /// The target is reached: this is the settled picture.
    last: bool,
    /// Denoise the settled picture.
    denoise: bool,
}

/// Everything behind `Shared::inner`.
struct Inner {
    control: Control,
    phase: Phase,
    samples: u32,
    target: u32,
    epoch: Option<Epoch>,
    /// The planner of the epoch before, kept for its measured chunk times.
    prev_planner: Option<ChunkPlanner>,
    /// Measured pixel-samples per millisecond of one thread.
    rate: Option<f64>,
}

impl Inner {
    /// The next chunk for thread `index`, if it may trace and there is one.
    fn take_job(&mut self, index: usize) -> Option<Job> {
        if self.phase != Phase::Active
            || self.control.paused_reason().is_some()
            || index >= self.control.workers_allowed
        {
            return None;
        }
        let epoch = self.epoch.as_mut()?;
        let partitions = epoch.planner.partitions();
        let completed = epoch.acc.completed_passes();
        for step in 0..partitions {
            let partition = (epoch.cursor + step) % partitions;
            let slot = partition as usize;
            if epoch.busy.get(slot).copied().unwrap_or(true) {
                continue;
            }
            if let Some(assignment) = epoch.planner.next_chunk(partition, completed) {
                epoch.cursor = (partition + 1) % partitions;
                epoch.busy[slot] = true;
                return Some(Job {
                    backend: epoch.backend.clone(),
                    cancel: Arc::clone(&epoch.cancel),
                    epoch_id: epoch.id,
                    assignment,
                    scene: Arc::clone(&epoch.scene),
                });
            }
        }
        None
    }

    /// Takes the result of a chunk. Returns the frame to show when it completed a pass.
    fn on_chunk(
        &mut self,
        job: &Job,
        sums: Option<Vec<Vec3>>,
        elapsed: Duration,
    ) -> Option<Follow> {
        let epoch = self
            .epoch
            .as_mut()
            .filter(|epoch| epoch.id == job.epoch_id)?;
        let assignment = job.assignment;
        if let Some(busy) = epoch.busy.get_mut(assignment.first_pixel as usize) {
            *busy = false;
        }
        let Some(sums) = sums else {
            // Aborted: hand the chunk back so a resumed epoch traces it.
            epoch.planner.unassign(&assignment);
            return None;
        };
        let millis = elapsed.as_secs_f64() * 1000.0;
        epoch
            .planner
            .record_timing(assignment.first_pixel, assignment.spp, millis);
        let work = sums.len() as f64 * f64::from(assignment.spp);
        let outcome = epoch.acc.add_chunk(
            epoch.id,
            assignment.first_pixel,
            assignment.stride,
            assignment.sample_offset,
            assignment.spp,
            sums,
        );
        let mut committed = false;
        match outcome {
            ChunkOutcome::Committed { .. } => {
                committed = true;
                epoch.finished = epoch.planner.is_done(epoch.acc.completed_passes());
                self.samples = epoch.acc.sample_count();
                if epoch.finished {
                    self.phase = Phase::Done;
                }
            }
            ChunkOutcome::Rejected(reason) => {
                tracing::warn!(?reason, "the render accumulator refused a chunk");
                self.phase = Phase::Failed(RUN_FAILED.to_owned());
            }
            ChunkOutcome::Pending | ChunkOutcome::Stale => {}
        }
        if millis > 0.0 && work > 0.0 {
            let sample = work / millis;
            self.rate = Some(
                self.rate
                    .map_or(sample, |old| RATE_SMOOTHING.mul_add(sample - old, old)),
            );
        }
        if committed { self.snapshot() } else { None }
    }

    /// A copy of the merged frame of the current epoch.
    fn snapshot(&self) -> Option<Follow> {
        let epoch = self.epoch.as_ref()?;
        Some(Follow {
            epoch_id: epoch.id,
            obsolete: Arc::clone(&epoch.obsolete),
            width: epoch.acc.width(),
            height: epoch.acc.height(),
            samples: epoch.acc.sample_count(),
            target: epoch.target,
            sum: epoch.acc.sum().to_vec(),
            scene: Arc::clone(&epoch.scene),
            last: epoch.finished,
            denoise: self.control.denoise && epoch.scene.engine().denoises(),
        })
    }
}

/// What Render Details needs to know about the latest GPU preparation.
#[derive(Default)]
struct PrepTrack {
    /// `process_ms()` when a preparing status was first seen, while one is on.
    began_ms: Option<u64>,
    /// The step, and its counts, seen last while preparing.
    last_step: Option<(diag::PrepareStep, u32, u32)>,
    /// The preparation that ended, or was abandoned.
    last: Option<PreparationTime>,
}

/// The newest frame and what was published last, for the "never go backwards" rule.
#[derive(Default)]
struct FrameSlot {
    latest: Option<RenderFrame>,
    /// `(epoch, samples, denoised)` of the newest frame ever published.
    last: Option<(u64, u32, bool)>,
}

/// The state every render thread and the owning `RenderState` share.
pub(super) struct Shared {
    pub(super) backend: BackendManager,
    pub(super) policy: Arc<crate::compute_policy::ComputePolicy>,
    inner: Mutex<Inner>,
    wake: Condvar,
    frames: Mutex<FrameSlot>,
    /// The epoch the threads must trace; 0 when none.
    current_epoch: AtomicU64,
    /// Tracing is paused: running chunks stop at their next check.
    halt: AtomicBool,
    /// The pool is shutting down.
    closed: AtomicBool,
    denoiser: Mutex<Denoiser>,
    /// The `render-aux` queue is inside a scene preparation (the GPU may be starting).
    aux_preparing: AtomicBool,
    /// Names the preparation in flight; an abandon changes it.
    aux_token: AtomicU64,
    /// The latest preparation's time, for Render Details.
    preparation: Mutex<PrepTrack>,
    /// Per render thread: it is inside a trace on the GPU (a kernel may be compiling).
    worker_gpu: Vec<AtomicBool>,
    /// Per render thread index: the generation of the thread that owns the index. A thread
    /// whose generation was replaced (an abandoned one) leaves its loop when it comes back.
    worker_generation: Vec<AtomicU64>,
    /// A start was asked for while the GPU probe held renders back.
    start_asked: AtomicBool,
    /// Test view: how many denoises started.
    #[cfg(test)]
    denoise_calls: AtomicU64,
}

impl Shared {
    /// A pool state for `max_workers` threads (none spawned yet).
    pub(super) fn new(max_workers: usize) -> Self {
        Self {
            backend: BackendManager::new(),
            policy: Arc::new(crate::compute_policy::ComputePolicy::new(max_workers)),
            inner: Mutex::new(Inner {
                control: Control {
                    max_workers,
                    workers_allowed: max_workers,
                    user_paused: false,
                    background: false,
                    hot: false,
                    denoise: true,
                },
                phase: Phase::Idle,
                samples: 0,
                target: 0,
                epoch: None,
                prev_planner: None,
                rate: None,
            }),
            wake: Condvar::new(),
            frames: Mutex::new(FrameSlot::default()),
            current_epoch: AtomicU64::new(0),
            halt: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            denoiser: Mutex::new(Denoiser::single_thread()),
            aux_preparing: AtomicBool::new(false),
            aux_token: AtomicU64::new(0),
            preparation: Mutex::new(PrepTrack::default()),
            worker_gpu: (0..max_workers).map(|_| AtomicBool::new(false)).collect(),
            worker_generation: (0..max_workers).map(|_| AtomicU64::new(0)).collect(),
            start_asked: AtomicBool::new(false),
            #[cfg(test)]
            denoise_calls: AtomicU64::new(0),
        }
    }

    /// Notes that the `render-aux` queue entered a scene preparation. Returns the token the
    /// matching [`Self::end_aux_prepare`] must present.
    pub(super) fn begin_aux_prepare(&self) -> u64 {
        let token = self.aux_token.load(Ordering::Acquire);
        self.aux_preparing.store(true, Ordering::Release);
        token
    }

    /// Notes that the preparation of `token` ended. An abandoned preparation that ends late
    /// presents an old token and changes nothing.
    pub(super) fn end_aux_prepare(&self, token: u64) {
        if self.aux_token.load(Ordering::Acquire) == token {
            self.aux_preparing.store(false, Ordering::Release);
        }
    }

    /// Whether the `render-aux` queue is inside a scene preparation.
    #[cfg(test)]
    pub(super) fn aux_preparing(&self) -> bool {
        self.aux_preparing.load(Ordering::Acquire)
    }

    /// Gives up the scene preparation in flight: its token is void from now on. Returns
    /// whether there was one.
    pub(super) fn abandon_aux_prepare(&self) -> bool {
        let was = self.aux_preparing.swap(false, Ordering::AcqRel);
        if was {
            self.aux_token.fetch_add(1, Ordering::AcqRel);
        }
        was
    }

    /// The render thread indexes that are inside a GPU trace now.
    pub(super) fn gpu_workers(&self) -> Vec<usize> {
        self.worker_gpu
            .iter()
            .enumerate()
            .filter(|(_, busy)| busy.load(Ordering::Acquire))
            .map(|(index, _)| index)
            .collect()
    }

    /// Marks thread `index` as inside (or out of) a GPU trace, if `generation` still owns it.
    pub(super) fn set_worker_gpu(&self, index: usize, generation: u64, busy: bool) {
        if self.worker_generation(index) == generation
            && let Some(flag) = self.worker_gpu.get(index)
        {
            flag.store(busy, Ordering::Release);
        }
    }

    /// The generation of the thread that owns render thread `index` (0 for the first).
    pub(super) fn worker_generation(&self, index: usize) -> u64 {
        self.worker_generation
            .get(index)
            .map_or(0, |generation| generation.load(Ordering::Acquire))
    }

    /// Gives render thread `index` to a new thread: the old one leaves its loop when its
    /// trace ends. Returns the new generation.
    pub(super) fn supersede_worker(&self, index: usize) -> u64 {
        if let Some(flag) = self.worker_gpu.get(index) {
            flag.store(false, Ordering::Release);
        }
        self.worker_generation.get(index).map_or(0, |generation| {
            generation.fetch_add(1, Ordering::AcqRel) + 1
        })
    }

    /// Remembers that a start was asked for while the probe held renders back.
    pub(super) fn remember_start(&self) {
        self.start_asked.store(true, Ordering::Release);
    }

    /// Whether a start was remembered; clears it.
    pub(super) fn take_remembered_start(&self) -> bool {
        self.start_asked.swap(false, Ordering::AcqRel)
    }

    /// Whether the pool was shut down.
    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Whether `epoch` is still the one to trace; work for a superseded epoch is discarded.
    pub(super) fn is_current(&self, epoch: u64) -> bool {
        !self.is_closed() && self.current_epoch.load(Ordering::Acquire) == epoch
    }

    /// Ends the pool: the threads leave their loops.
    pub(super) fn close(&self) {
        self.policy.close();
        self.closed.store(true, Ordering::Release);
        self.current_epoch.store(0, Ordering::Release);
        {
            let mut inner = relock(&self.inner);
            if let Some(epoch) = &inner.epoch {
                epoch.cancel.store(true, Ordering::Release);
                epoch.obsolete.store(true, Ordering::Release);
            }
            inner.epoch = None;
            inner.phase = Phase::Idle;
        }
        self.clear_frames();
        self.wake.notify_all();
    }

    /// Whether a chunk of `epoch` should stop now.
    fn abort_requested(&self, epoch: u64) -> bool {
        self.is_closed()
            || self.halt.load(Ordering::Acquire)
            || self.current_epoch.load(Ordering::Acquire) != epoch
    }

    fn clear_frames(&self) {
        *relock(&self.frames) = FrameSlot::default();
    }

    /// Changes the controls under the lock, refreshes the halt flag and wakes the threads.
    pub(super) fn update_control(&self, edit: impl FnOnce(&mut Control)) {
        {
            let mut inner = relock(&self.inner);
            edit(&mut inner.control);
            let paused = inner.control.paused_reason().is_some();
            if let Some(epoch) = &inner.epoch {
                epoch.cancel.store(paused, Ordering::Release);
            }
            self.halt
                .store(inner.control.paused_reason().is_some(), Ordering::Release);
        }
        self.wake.notify_all();
    }

    /// The controls as they stand.
    pub(super) fn control(&self) -> Control {
        relock(&self.inner).control.clone()
    }

    /// Starts epoch `id` towards `target` samples; the scene follows through
    /// [`Self::install_scene`].
    pub(super) fn begin_epoch(&self, id: u64, target: u32, denoise: bool) {
        self.backend.begin(id);
        self.current_epoch.store(id, Ordering::Release);
        {
            let mut inner = relock(&self.inner);
            if let Some(old) = inner.epoch.take() {
                old.cancel.store(true, Ordering::Release);
                old.obsolete.store(true, Ordering::Release);
                inner.prev_planner = Some(old.planner);
            }
            inner.phase = Phase::Active;
            inner.samples = 0;
            inner.target = target;
            inner.control.denoise = denoise;
            self.halt
                .store(inner.control.paused_reason().is_some(), Ordering::Release);
        }
        self.clear_frames();
        self.wake.notify_all();
    }

    /// Hands epoch `id` its built scene (or the reason there is none). Dropped when the
    /// epoch was replaced meanwhile.
    pub(super) fn install_scene(&self, id: u64, built: Result<OwnedScene, String>, target: u32) {
        if !self.is_current(id) {
            return;
        }
        let built = built.and_then(|scene| {
            self.backend
                .prepare(&scene, id)
                .map(|backend| (scene, backend))
        });
        {
            let mut inner = relock(&self.inner);
            if self.is_closed() || self.current_epoch.load(Ordering::Acquire) != id {
                return;
            }
            match built {
                Ok((scene, backend)) => {
                    let (width, height) = (scene.width(), scene.height());
                    let partitions = if backend.is_gpu() {
                        1
                    } else {
                        u32::try_from(inner.control.max_workers).unwrap_or(1).max(1)
                    };
                    let pixels = width.saturating_mul(height);
                    let planner = inner.prev_planner.take().map_or_else(
                        || ChunkPlanner::new(partitions, pixels, target),
                        |previous| {
                            ChunkPlanner::with_rates_from(&previous, partitions, pixels, target)
                        },
                    );
                    inner.epoch = Some(Epoch {
                        backend,
                        cancel: Arc::new(AtomicBool::new(inner.control.paused_reason().is_some())),
                        obsolete: Arc::new(AtomicBool::new(false)),
                        id,
                        scene: Arc::new(scene),
                        planner,
                        acc: Accumulator::new(id, width, height, partitions),
                        cursor: 0,
                        busy: vec![false; partitions as usize],
                        target,
                        finished: false,
                    });
                }
                Err(message) => inner.phase = Phase::Failed(message),
            }
        }
        self.wake.notify_all();
    }

    /// Remote display images are published independently of the local radiance ledger.
    pub(super) fn remote_frame(
        &self,
        id: u64,
        width: u32,
        height: u32,
        target: u32,
        update: crate::remote::rendering::FrameUpdate,
    ) {
        if !self.is_current(id) || update.samples > target {
            return;
        }
        {
            let mut inner = relock(&self.inner);
            if !self.is_current(id) {
                return;
            }
            inner.samples = inner.samples.max(update.samples);
            if update.done {
                inner.phase = Phase::Done;
            }
        }
        if let Some(rgba) = update.rgba {
            if rgba.len() != width as usize * height as usize * 4 {
                return;
            }
            self.publish(
                RenderFrame {
                    epoch: id,
                    width,
                    height,
                    rgba,
                    samples: update.samples,
                    target,
                    denoised: update.denoised,
                },
                false,
            );
        }
    }
    pub(super) fn remote_failed(&self, id: u64, message: &str) {
        self.fail(id, message);
    }

    /// A remote session starts over from sample zero (a reconnect): the sample count and
    /// the newest-frame guard, which drops frames with fewer samples than the last one,
    /// are reset so the restarted render is shown from its first picture. The previous
    /// picture stays visible until then.
    pub(super) fn remote_restarted(&self, id: u64) {
        if !self.is_current(id) {
            return;
        }
        relock(&self.inner).samples = 0;
        relock(&self.frames).last = None;
    }

    /// Cancels the epoch and goes idle, keeping the threads.
    pub(super) fn stop(&self) {
        self.current_epoch.store(0, Ordering::Release);
        {
            let mut inner = relock(&self.inner);
            if let Some(old) = inner.epoch.take() {
                old.cancel.store(true, Ordering::Release);
                old.obsolete.store(true, Ordering::Release);
                inner.prev_planner = Some(old.planner);
            }
            inner.phase = Phase::Idle;
            inner.samples = 0;
        }
        self.clear_frames();
        self.wake.notify_all();
    }

    /// Cancels the epoch and reports `message` as the reason there is nothing to show.
    pub(super) fn fail_now(&self, message: &str) {
        self.stop();
        relock(&self.inner).phase = Phase::Failed(message.to_owned());
    }

    /// Marks the running epoch `id` as failed (a thread panicked).
    pub(super) fn fail(&self, id: u64, message: &str) {
        let mut inner = relock(&self.inner);
        if self.is_current(id) {
            inner.phase = Phase::Failed(message.to_owned());
        }
    }

    /// The pool's status.
    ///
    /// `Preparing` is returned only when the epoch is local, nothing is merged yet and the
    /// process-wide snapshot shows a GPU step in flight (or the preview holds one on show);
    /// every other `Tracing { samples: 0 }` stays as it was.
    pub(super) fn status(&self) -> RenderStatus {
        let status = self.status_with(
            &diag::prepare_progress(),
            diag::prepare_override().is_some(),
        );
        self.track_preparation(&status);
        status
    }

    /// Notes what `status` says about a preparation: when it began, and its time once it
    /// ended. The UI's polling of the status is what drives it, so the time is as exact as
    /// the poll interval.
    fn track_preparation(&self, status: &RenderStatus) {
        let now = diag::process_ms();
        let mut track = relock(&self.preparation);
        if let RenderStatus::Preparing {
            step, done, total, ..
        } = status
        {
            track.began_ms.get_or_insert(now);
            track.last_step = Some((*step, *done, *total));
        } else if let Some(began) = track.began_ms.take() {
            track.last_step = None;
            track.last = Some(PreparationTime {
                seconds: u32::try_from(now.saturating_sub(began) / 1000).unwrap_or(u32::MAX),
                stopped_in: None,
            });
        }
    }

    /// Records that the preparation in flight was abandoned, with the step it stopped in.
    pub(super) fn note_preparation_abandoned(&self) {
        let now = diag::process_ms();
        let progress = diag::prepare_progress();
        let mut track = relock(&self.preparation);
        let began = track.began_ms.take().unwrap_or(progress.start_ms);
        let stopped_in = track.last_step.take().or_else(|| {
            (progress.step != diag::PrepareStep::Idle).then_some((
                progress.step,
                progress.done,
                progress.total,
            ))
        });
        track.last = Some(PreparationTime {
            seconds: u32::try_from(now.saturating_sub(began) / 1000).unwrap_or(u32::MAX),
            stopped_in,
        });
    }

    /// The latest preparation that ended or was abandoned.
    pub(super) fn last_preparation(&self) -> Option<PreparationTime> {
        relock(&self.preparation).last.clone()
    }

    /// [`Self::status`] for a given snapshot of the GPU preparation; `held` says the preview
    /// holds a frozen preparing state on show.
    pub(super) fn status_with(&self, progress: &diag::PrepareProgress, held: bool) -> RenderStatus {
        if held {
            return preparing_status(progress);
        }
        let inner = relock(&self.inner);
        match &inner.phase {
            Phase::Idle => RenderStatus::Idle,
            Phase::Failed(message) => RenderStatus::Failed(message.clone()),
            Phase::Done => RenderStatus::Done {
                samples: inner.samples,
            },
            Phase::Active => inner.control.paused_reason().map_or_else(
                || {
                    // A CPU epoch starts no GPU step of its own: a step still in flight then
                    // belongs to an abandoned thread.
                    if inner.samples == 0
                        && progress.step != diag::PrepareStep::Idle
                        && !matches!(
                            self.backend.requested(),
                            RenderBackend::Remote | RenderBackend::DeviceCpu
                        )
                    {
                        preparing_status(progress)
                    } else {
                        RenderStatus::Tracing {
                            samples: inner.samples,
                            target: inner.target,
                        }
                    }
                },
                RenderStatus::Paused,
            ),
        }
    }

    /// The measured rate of one thread in pixel-samples per millisecond.
    pub(super) fn rate_per_thread(&self) -> Option<f64> {
        relock(&self.inner).rate
    }

    /// The newest frame of the current epoch, once.
    pub(super) fn take_frame(&self) -> Option<RenderFrame> {
        let current = self.current_epoch.load(Ordering::Acquire);
        relock(&self.frames)
            .latest
            .take()
            .filter(|frame| frame.epoch == current)
    }

    /// Switches denoise. Returns the settled frame to re-show when the epoch is done.
    pub(super) fn set_denoise(&self, on: bool) -> Option<Follow> {
        let mut inner = relock(&self.inner);
        inner.control.denoise = on;
        if inner.phase == Phase::Done {
            inner.snapshot()
        } else {
            None
        }
    }

    /// Shows the settled picture of `follow`, denoised or plain, whatever was shown.
    pub(super) fn show_settled(&self, follow: &Follow, denoised: bool) {
        if denoised && follow.scene.engine().denoises() {
            self.show_denoised(follow, true);
        } else {
            self.publish(plain_frame(follow), true);
        }
    }

    /// Publishes `frame` unless a newer picture of its epoch was published before.
    fn publish(&self, frame: RenderFrame, force: bool) {
        let mut slots = relock(&self.frames);
        if self.current_epoch.load(Ordering::Acquire) != frame.epoch {
            return;
        }
        if !force
            && let Some((epoch, samples, denoised)) = slots.last
            && epoch == frame.epoch
            && (samples > frame.samples
                || (samples == frame.samples && denoised && !frame.denoised))
        {
            return;
        }
        slots.last = Some((frame.epoch, frame.samples, frame.denoised));
        slots.latest = Some(frame);
    }

    /// Denoises and publishes the settled picture of `follow`, unless its epoch ended.
    ///
    /// The epoch's `obsolete` flag is checked while waiting for the permit, again after
    /// the `denoiser` lock is won (a newer request therefore never works for an obsolete
    /// one), and inside the denoise itself: the owner of an obsolete epoch lets go of the
    /// permit and the lock within one guide row or one filter block, and publishes nothing.
    fn show_denoised(&self, follow: &Follow, force: bool) {
        let obsolete =
            || follow.obsolete.load(Ordering::Acquire) || !self.is_current(follow.epoch_id);
        let Some(_permit) = self.policy.acquire(&obsolete) else {
            return;
        };
        let picture = catch_unwind(AssertUnwindSafe(|| {
            let mut denoiser = relock(&self.denoiser);
            if obsolete() {
                return None;
            }
            #[cfg(test)]
            self.denoise_calls.fetch_add(1, Ordering::Relaxed);
            denoiser.denoised_rgba_cancellable(
                &DenoiseFrame {
                    guide_key: follow.epoch_id,
                    width: follow.width,
                    height: follow.height,
                    camera: follow.scene.camera(),
                    planes: follow.scene.planes(),
                    tools: follow.scene.tools(),
                    n_d: follow.scene.material().dispersion.n_d(),
                    sample_count: follow.samples,
                    sum: &follow.sum,
                },
                &follow.obsolete,
            )
        }));
        match picture {
            Ok(Some(rgba)) => self.publish(
                RenderFrame {
                    rgba,
                    denoised: true,
                    ..plain_frame_without_pixels(follow)
                },
                force,
            ),
            // Cancelled: the epoch is over, there is nothing to publish.
            Ok(None) => {}
            Err(_) => tracing::warn!("the denoiser panicked; the plain picture stays"),
        }
    }

    /// Shows a merged pass: the plain picture, then the denoised one when it is the last.
    fn show(&self, follow: &Follow) {
        self.publish(plain_frame(follow), false);
        if follow.last && follow.denoise {
            self.show_denoised(follow, false);
        }
    }

    /// Takes the result of a traced chunk and shows what it completed.
    fn complete(&self, job: &Job, sums: Option<Vec<Vec3>>, elapsed: Duration) {
        let follow = relock(&self.inner).on_chunk(job, sums, elapsed);
        self.wake.notify_all();
        if let Some(follow) = follow {
            // One line per GPU range (a whole-frame range of one or more samples) and one for
            // the end of the render; never one per sample or per CPU chunk.
            if job.backend.is_gpu() {
                tracing::info!(
                    target: diag::TRACER_TARGET,
                    samples = follow.samples,
                    target_samples = follow.target,
                    ms_per_sample = elapsed.as_secs_f64() * 1000.0 / f64::from(job.assignment.spp.max(1)),
                    "range done"
                );
            }
            if follow.last {
                tracing::info!(
                    target: diag::TRACER_TARGET,
                    samples = follow.samples,
                    "render done"
                );
            }
            self.show(&follow);
        }
    }

    /// Blocks until there is a chunk for thread `index`; `None` when the pool is closed or a
    /// newer thread (`generation` is outdated) owns the index.
    fn next_job(&self, index: usize, generation: u64) -> Option<Job> {
        let mut inner = relock(&self.inner);
        loop {
            if self.is_closed() || self.worker_generation(index) != generation {
                return None;
            }
            if let Some(job) = inner.take_job(index) {
                return Some(job);
            }
            inner = self
                .wake
                .wait_timeout(inner, IDLE_WAIT)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Test view: pretends `samples` passes are merged.
    #[cfg(test)]
    pub(super) fn debug_set_samples(&self, samples: u32) {
        relock(&self.inner).samples = samples;
    }

    /// The test view of the running epoch's merged sum and sample count.
    #[cfg(test)]
    pub(super) fn debug_sum(&self) -> Option<(u32, Vec<Vec3>)> {
        let inner = relock(&self.inner);
        inner
            .epoch
            .as_ref()
            .map(|epoch| (epoch.acc.sample_count(), epoch.acc.sum().to_vec()))
    }
}

/// The status of a GPU step in flight: its step, counts and whole seconds so far.
fn preparing_status(progress: &diag::PrepareProgress) -> RenderStatus {
    let elapsed_ms = diag::process_ms().saturating_sub(progress.start_ms);
    RenderStatus::Preparing {
        step: progress.step,
        done: progress.done,
        total: progress.total,
        elapsed_s: u32::try_from(elapsed_ms / 1000).unwrap_or(u32::MAX),
    }
}

/// The tone-mapped mean of the merged sum.
fn mean_rgba(sum: &[Vec3], samples: u32) -> Vec<u8> {
    // One thread: the pool's own threads are what the governor budgets.
    tonemap_to_rgba_with_threads(sum, 1.0 / samples.max(1) as f32, 1)
}

/// The plain picture of `follow`.
fn plain_frame(follow: &Follow) -> RenderFrame {
    RenderFrame {
        rgba: mean_rgba(&follow.sum, follow.samples),
        ..plain_frame_without_pixels(follow)
    }
}

/// A frame record of `follow` with no pixels yet.
const fn plain_frame_without_pixels(follow: &Follow) -> RenderFrame {
    RenderFrame {
        epoch: follow.epoch_id,
        width: follow.width,
        height: follow.height,
        rgba: Vec::new(),
        samples: follow.samples,
        target: follow.target,
        denoised: false,
    }
}

/// The body of render thread `index`: trace chunks until the pool closes, or until a newer
/// thread took the index over (`generation` is no longer the index's own).
pub(super) fn worker_loop(shared: &Shared, index: usize, generation: u64) {
    while let Some(job) = shared.next_job(index, generation) {
        let assignment = job.assignment;
        let range = ChunkRange {
            first_pixel: assignment.first_pixel,
            stride: assignment.stride,
            sample_offset: assignment.sample_offset,
            spp: assignment.spp,
        };
        let aborted = || shared.abort_requested(job.epoch_id);
        let started = Instant::now();
        let on_gpu = job.backend.is_gpu();
        let traced = catch_unwind(AssertUnwindSafe(|| {
            let _permit = shared.policy.acquire(&aborted)?;
            // Only a thread that holds its permit can be stuck in a GPU compile.
            shared.set_worker_gpu(index, generation, on_gpu);
            shared.backend.trace(
                &job.backend,
                &job.scene,
                range,
                &job.cancel,
                &aborted,
                job.epoch_id,
            )
        }));
        shared.set_worker_gpu(index, generation, false);
        let elapsed = started.elapsed();
        // Merging and showing the chunk can panic too (a tone-map or accumulator bug); it
        // must not end this thread, or the pool would shrink with every such panic.
        let finished = traced.and_then(|sums| {
            catch_unwind(AssertUnwindSafe(|| shared.complete(&job, sums, elapsed)))
        });
        if finished.is_err() {
            tracing::error!("a render thread panicked while tracing");
            shared.fail(job.epoch_id, RUN_FAILED);
        }
    }
}

#[cfg(test)]
mod denoise_cancel_tests;
