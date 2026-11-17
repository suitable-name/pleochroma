//! [`GpuBackend`], the `feature = "gpu"` implementation of the decline-and-fall-back
//! wrapper the parent module's doc comment describes. [`super::stub::GpuBackend`] is the
//! signature-compatible stand-in compiled instead when the feature is off.

use std::{
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use glam::Vec3;

use super::{
    GpuAccumulate, GpuPipelineKind, GpuSceneRef,
    recovery::{COOL_DOWN, RecoveryPolicy},
    turnstile::{CHUNKS_PER_TURN, Turnstile},
};
use crate::{
    geometry::tool::{GPU_MAX_TOOL_PRIMITIVES, ToolPrimitive},
    renderer::{
        buffers::GpuSurfaceScene,
        gpu::{
            GpuFrameError, GpuFrameRenderer,
            frame::{BatchState, ChunkCursor, ChunkTurnOutcome, TurnRequest, classify_material},
            timing::{self, PassSlot, TurnSlot},
        },
    },
};

mod batch;
mod batch_output;
#[cfg(feature = "beam")]
mod beam;
mod cached;
mod live;
#[cfg(feature = "beam")]
mod reference;
mod resident;
mod warmup;
use batch_output::BatchOutput;
pub use resident::GpuResidentRun;

/// The most scratch buffers [`GpuBackend`] keeps between requests. Each is as large as
/// the biggest frame it served (12 bytes a pixel), so the pool is kept small; requests
/// beyond it allocate their own and drop it afterwards.
const MAX_POOLED_SCRATCH: usize = 2;

/// The GPU render backend: one renderer behind a mutex, with loss tracking and recovery.
pub struct GpuBackend {
    pub(super) limits: Mutex<Option<super::GpuWorkLimits>>,
    /// `pub(super)` so `super::tests` can poison the mutex from a test thread.
    ///
    /// `Some` exactly when an adapter was acquired at construction. After a loss,
    /// [`Self::try_recover`] replaces the renderer inside the mutex; the mutex and this
    /// `Option` never change.
    pub(super) renderer: Option<Mutex<GpuFrameRenderer>>,
    /// FIFO admission for each turn of [`Self::try_accumulate_cancellable`]; see the parent
    /// module's "Concurrency" section. Always present, even with no `renderer`, so the
    /// struct needs no `Option` around it.
    turnstile: Turnstile,
    /// Set when a dispatch reports [`GpuFrameError::DeviceLost`] or finds the renderer mutex
    /// poisoned; cleared only by a successful [`Self::try_recover`]. While set, calls decline
    /// without touching the renderer, since every dispatch on the same device would fail the
    /// same way. `AtomicBool` so it can be read without the `renderer` mutex.
    ///
    /// `pub(super)` so `super::tests` can check that a poisoned mutex sets it.
    pub(super) lost: AtomicBool,
    /// Why [`Self::lost`] was last set: the `why` text of a [`GpuFrameError::DeviceLost`], or
    /// a fixed message for a poisoned mutex. `None` until the first loss; kept after a
    /// recovery. Read by [`Self::last_lost_reason`]. A `Mutex` because the payload is a
    /// `String`; it is only touched on the loss path.
    last_lost_reason: Mutex<Option<String>>,
    /// The current renderer's adapter label, kept outside the `renderer` mutex so
    /// [`Self::adapter_label`] never waits for a render (a first-time shader compile holds that
    /// mutex for minutes, and status pages ask for the label every second). Set at acquisition
    /// and by [`Self::reacquire`].
    label: Mutex<Option<String>>,
    /// Cool-down and attempt budget for [`Self::try_recover`]; see [`RecoveryPolicy`].
    /// `pub(super)` so tests can age a loss without sleeping.
    pub(super) recovery: Mutex<RecoveryPolicy>,
    /// `true` while one thread is inside [`Self::reacquire`], so concurrent callers decline
    /// instead of queueing up behind a slow device acquisition.
    recovering: AtomicBool,
    /// Zeroed scratch buffers reused across requests. A request traces into one and adds it
    /// to the caller's `accum` only when it completes; see [`Self::try_accumulate_cancellable`].
    /// A pool, because concurrent requests interleave their turns and each needs its own
    /// partial sums.
    scratch_pool: Mutex<Vec<Vec<Vec3>>>,
    /// Test seam: the 1-based turn number, counted per request, whose outcome is replaced
    /// by an injected [`GpuFrameError::DeviceLost`] (`0` = never). Clears itself when it fires.
    #[cfg(test)]
    pub(super) fail_on_turn: std::sync::atomic::AtomicUsize,
}

impl GpuBackend {
    /// Whether the current renderer can preserve the stone shadow on an ordinary production pass.
    #[must_use]
    pub fn supports_tray_shadow(&self) -> bool {
        if self.is_lost() {
            return false;
        }
        self.renderer
            .as_ref()
            .and_then(|renderer| renderer.lock().ok())
            .is_some_and(|renderer| renderer.supports_tray_shadow())
    }

    /// Whether the current renderer can trace the Tray Reflection on an ordinary production pass.
    ///
    /// Delegates to [`Self::supports_tray_shadow`]: the two features ride the same tray variant of
    /// the megakernel and the same binding 190, and the renderer's admission ignores which tray
    /// bit is set, so the conditions are identical.
    #[must_use]
    pub fn supports_tray_mirror(&self) -> bool {
        self.supports_tray_shadow()
    }

    /// Never acquires an adapter -- every call declines, regardless of what hardware is
    /// present.
    ///
    /// The runtime opt-out (`pleochroma-worker`'s `--only-cpu` flag), and what tests construct
    /// instead of [`Self::acquire`], so a `gpu` build stays as deterministic as a plain one.
    #[must_use]
    pub const fn disabled() -> Self {
        Self::with_renderer(None)
    }

    /// The one place a backend's fields are initialised.
    pub(super) const fn with_renderer(renderer: Option<Mutex<GpuFrameRenderer>>) -> Self {
        Self {
            limits: Mutex::new(None),
            renderer,
            turnstile: Turnstile::new(),
            lost: AtomicBool::new(false),
            last_lost_reason: Mutex::new(None),
            label: Mutex::new(None),
            recovery: Mutex::new(RecoveryPolicy::new()),
            recovering: AtomicBool::new(false),
            scratch_pool: Mutex::new(Vec::new()),
            #[cfg(test)]
            fail_on_turn: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Whether the GPU device was lost and not yet recovered. Set by a
    /// [`GpuFrameError::DeviceLost`] or a poisoned renderer mutex; cleared when
    /// [`Self::try_recover`] acquires a fresh device. Unsupported scenes and
    /// [`Self::disabled`] backends do not set it. A caller that re-acquires after a `false`
    /// from [`Self::try_accumulate`] checks this first.
    #[must_use]
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// The reason for the most recent loss: the `why` text of a [`GpuFrameError::DeviceLost`],
    /// or a fixed message for a poisoned mutex. `None` before any loss and for a
    /// [`Self::disabled`] backend. Still returned after a recovery.
    #[must_use]
    pub fn last_lost_reason(&self) -> Option<String> {
        self.last_lost_reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// This backend's adapter label, if an adapter was acquired.
    ///
    /// `None` when the backend is [`Self::disabled`], acquisition failed, or the device is
    /// lost (see [`Self::lost`]). Read-only: it never attempts recovery, so a caller about to
    /// advertise a GPU calls [`Self::try_recover`] first.
    ///
    /// Never waits for a render: the label is kept outside the renderer mutex. A backend built
    /// without it (tests) fills it on the first call that finds the renderer free.
    #[must_use]
    pub fn adapter_label(&self) -> Option<String> {
        if self.lost.load(Ordering::Relaxed) {
            return None;
        }
        let renderer = self.renderer.as_ref()?;
        let mut label = self.label.lock().unwrap_or_else(PoisonError::into_inner);
        if label.is_none()
            && let Ok(guard) = renderer.try_lock()
        {
            *label = Some(guard.adapter_label().to_owned());
        }
        label.clone()
    }

    /// Remembers the current renderer's adapter label for [`Self::adapter_label`].
    pub(super) fn remember_label(&self, label: String) {
        *self.label.lock().unwrap_or_else(PoisonError::into_inner) = Some(label);
    }

    /// Selects the kernel used for every later chunk; see [`GpuPipelineKind`]. A no-op
    /// without a renderer (see [`Self::disabled`]). The choice carries over to a renderer
    /// re-acquired after a device loss.
    pub fn set_pipeline_kind(&self, kind: GpuPipelineKind) {
        if let Some(mutex) = &self.renderer {
            mutex
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .set_pipeline_kind(kind);
        }
    }

    /// Traces `spp` samples per pixel starting at `sample_offset` and ADDS each pixel's
    /// summed XYZ into `accum`.
    ///
    /// A wrapper over [`Self::try_accumulate_cancellable`] with a flag that is never set, for
    /// callers without cancellation. Returns `false` and leaves `accum` untouched on a
    /// decline. The parent module's doc comment lists the decline reasons.
    pub fn try_accumulate(
        &self,
        scene: &GpuSceneRef<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> bool {
        let never_cancel = AtomicBool::new(false);
        matches!(
            self.try_accumulate_cancellable(scene, sample_offset, spp, accum, &never_cancel),
            GpuAccumulate::Done
        )
    }

    /// Like [`Self::try_accumulate`], but checks `cancel` before every chunk (see
    /// `GpuFrameRenderer::accumulate_turn` for exactly when) and can stop early,
    /// reporting which of three things happened via [`GpuAccumulate`] rather than a
    /// `bool`: [`GpuAccumulate::Done`], [`GpuAccumulate::Declined`] (`accum` untouched,
    /// fall back to the CPU for the full `spp`), or [`GpuAccumulate::Cancelled`] (`accum`
    /// untouched, nothing left to render).
    ///
    /// `accum` is written ONLY on `Done`. Every turn traces into a zeroed scratch buffer
    /// of the same length that this backend owns and reuses; the scratch is added into
    /// `accum` once the last turn reports done, and discarded otherwise. A device loss,
    /// an uncaptured wgpu error or a cancellation on a LATER turn therefore cannot leave
    /// the samples of earlier turns behind for the caller's CPU fallback to double count.
    /// Each pixel belongs to exactly one chunk, so the final add is one `+=` per pixel --
    /// the same sum a direct accumulation would have produced.
    ///
    /// Drives the renderer in turns of [`CHUNKS_PER_TURN`] chunks, so other callers sharing
    /// this backend can interleave. The parent module's "Concurrency" section gives the
    /// correctness argument (draining before yielding, per-turn scene re-upload, deterministic
    /// sample offsets).
    ///
    /// A device reported as lost ([`GpuFrameError::DeviceLost`]) is logged at `warn` (an
    /// ordinary decline is `debug`) and this backend's [`Self::lost`] flag is set so every
    /// later call short-circuits to `Declined` without touching the renderer -- until
    /// [`Self::try_recover`] (called here, at the top of each request, once the cool-down
    /// has passed) brings a fresh device back.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `scene.width * scene.height`.
    pub fn try_accumulate_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        self.try_accumulate_geom_cancellable(scene, &[], sample_offset, spp, accum, cancel)
    }

    /// [`Self::try_accumulate`] for a concave stone: the planes of `scene` minus `tools`.
    ///
    /// `tools` empty is exactly [`Self::try_accumulate`]. Tool `k` has facet id
    /// `scene.planes.len() + k` (`scene.facet_finishes` is indexed by facet id). Declines a
    /// stone with more than `GPU_MAX_TOOL_PRIMITIVES` tools; callers route first with
    /// [`super::scene_routes_to_gpu_with_tools`].
    pub fn try_accumulate_geom(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> bool {
        let never_cancel = AtomicBool::new(false);
        matches!(
            self.try_accumulate_geom_cancellable(
                scene,
                tools,
                sample_offset,
                spp,
                accum,
                &never_cancel
            ),
            GpuAccumulate::Done
        )
    }

    /// [`Self::try_accumulate_cancellable`] for a concave stone -- see
    /// [`Self::try_accumulate_geom`]. The tool records are packed after the planes in the
    /// `planes` storage binding for the whole request, across every turn.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `scene.width * scene.height`.
    pub fn try_accumulate_geom_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        self.accumulate_geom_surface(scene, tools, None, sample_offset, spp, accum, cancel)
    }

    /// The body of the `try_accumulate_geom*` family; `surface` `None` traces with the
    /// renderer's own (ideal by default) surface finish.
    #[expect(
        clippy::too_many_arguments,
        reason = "the cancellable geometry entry point's parameter list plus the surface"
    )]
    pub(super) fn accumulate_geom_surface(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: Option<&GpuSurfaceScene>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        if tools.len() > GPU_MAX_TOOL_PRIMITIVES {
            tracing::debug!(tools = tools.len(), "GPU declined: too many concave tools");
            return GpuAccumulate::Declined;
        }
        if !self.work_admits(scene, spp) {
            return GpuAccumulate::Declined;
        }
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
            return GpuAccumulate::Declined;
        }
        let Some(mutex) = &self.renderer else {
            return GpuAccumulate::Declined;
        };
        let gpu_scene = crate::renderer::gpu::GpuFrameScene {
            camera: scene.camera,
            width: scene.width,
            height: scene.height,
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            material: scene.material,
            max_bounces: scene.max_bounces,
            environment: scene.environment,
        };
        // The class the renderer derives for this scene. Computed once, outside the turn
        // loop: the material never changes across this request's turns.
        let pipeline_class = classify_material(gpu_scene.material);
        // Fixed for the whole request, across however many turns it takes to resume --
        // only the cursor and the scratch buffer change turn by turn. See TurnRequest's
        // own doc comment.
        let request = TurnRequest {
            scene: &gpu_scene,
            tools,
            pipeline_class,
            sample_offset,
            spp,
            cancel: Some(cancel),
            max_chunks: self
                .configured_limits()
                .map_or(CHUNKS_PER_TURN, |limits| limits.chunks_per_turn),
            surface,
            estimator: scene.estimator,
        };

        timing::begin_pass();
        let mut scratch_mark = timing::Mark::now();
        let mut scratch = self.take_scratch(accum.len());
        scratch_mark.lap_pass(PassSlot::ScratchTake);
        let outcome = self.run_turns(mutex, &request, &mut scratch);
        if outcome == GpuAccumulate::Done {
            let mut add_mark = timing::Mark::now();
            for (total, traced) in accum.iter_mut().zip(&scratch) {
                *total += *traced;
            }
            add_mark.lap_pass(PassSlot::ScratchAdd);
        }
        self.return_scratch(scratch);
        timing::end_pass(outcome == GpuAccumulate::Done);
        outcome
    }

    /// Runs `request` turn by turn, summing into `scratch` (never the caller's buffer --
    /// see [`Self::try_accumulate_cancellable`]), until it completes, is cancelled or
    /// declines. `scratch` holds partial sums after any outcome but `Done`; the caller
    /// discards it.
    fn run_turns(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        request: &TurnRequest<'_>,
        scratch: &mut [Vec3],
    ) -> GpuAccumulate {
        let mut cursor = ChunkCursor::default();
        #[cfg(test)]
        let mut turns_started = 0_usize;

        loop {
            // Admission is FIFO across every caller sharing this GpuBackend: a ticket is
            // taken here, at the moment this turn is ready to start, so a request that
            // rejoins the queue after finishing a turn goes to the BACK of it -- see
            // Turnstile's own doc comment.
            let mut wait_mark = timing::Mark::now();
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);

            // Re-checked after waiting: the turn ahead may have lost the device (the
            // `Err(DeviceLost)` arm below sets `self.lost`). Checked before `mutex.lock()`,
            // so this waiter declines instead of submitting into a lost renderer.
            if self.lost.load(Ordering::Acquire) {
                return GpuAccumulate::Declined;
            }

            // A poisoned mutex is never recovered with `PoisonError::into_inner`: the
            // renderer's state after a panic is unknown. It is treated as `DeviceLost`
            // instead: decline until `try_recover` replaces the renderer.
            let mut renderer = match mutex.lock() {
                Ok(guard) => guard,
                Err(_poisoned) => {
                    tracing::warn!(
                        "GPU renderer mutex poisoned (a previous turn panicked), disabling the \
                         GPU backend until a fresh device is acquired"
                    );
                    self.mark_lost("GPU renderer mutex poisoned (a previous turn panicked)".into());
                    return GpuAccumulate::Declined;
                }
            };
            wait_mark.lap_turn(TurnSlot::TurnstileWait);
            if let Some(limits) = self.configured_limits() {
                renderer.set_chunk_budget_bytes(limits.chunk_budget_bytes);
            }
            let outcome = renderer.accumulate_turn(request, scratch, &mut cursor);
            // Released before `_turn` ends, so a woken waiter never contends for a guard this
            // turn has finished with.
            drop(renderer);
            #[cfg(test)]
            let outcome = {
                turns_started += 1;
                self.injected_failure(turns_started).map_or(outcome, Err)
            };

            match outcome {
                Ok(ChunkTurnOutcome::Done) => return GpuAccumulate::Done,
                Ok(ChunkTurnOutcome::Cancelled) => return GpuAccumulate::Cancelled,
                // This turn's chunk budget is spent but pixels remain. `_turn` drops at the
                // bottom of this loop body, so the next waiter in FIFO order (this request
                // included, at the back) takes the next admission.
                Ok(ChunkTurnOutcome::MoreWork) => {}
                Err(GpuFrameError::DeviceLost(why)) => {
                    tracing::warn!(
                        "GPU device lost ({why}), disabling the GPU backend until a fresh \
                         device is acquired -- meanwhile every call falls back to the CPU \
                         tracer"
                    );
                    self.mark_lost(why);
                    return GpuAccumulate::Declined;
                }
                Err(e) => {
                    // Not worth `warn`: an unsupported material or environment is a
                    // documented limit of the megakernel, not a malfunction, and the CPU
                    // path produces the image either way.
                    tracing::debug!("GPU declined this dispatch, using CPU tracer: {e}");
                    return GpuAccumulate::Declined;
                }
            }
        }
    }

    /// The turn loop behind [`Self::try_accumulate_batch_cancellable`]: takes a ticket,
    /// steps the batch (one chunk per step, renderer lock taken per step so `on_done` never
    /// runs under it), and gives the turn up only when another request is waiting.
    fn run_batch(
        &self,
        mutex: &Mutex<GpuFrameRenderer>,
        requests: &[TurnRequest<'_>],
        output: &mut BatchOutput<'_, '_>,
    ) {
        let mut state = BatchState::new();
        #[cfg(test)]
        let mut steps_started = 0_usize;

        'turns: loop {
            let ticket = self.turnstile.take_ticket();
            let _turn = self.turnstile.wait_for_turn(ticket);

            // Re-checked after waiting, for the same reason as in `run_turns`.
            if self.lost.load(Ordering::Acquire) {
                output.finish_rest(self, GpuAccumulate::Declined);
                return;
            }
            let Some(mut renderer) = self.lock_renderer(mutex) else {
                output.finish_rest(self, GpuAccumulate::Declined);
                return;
            };
            renderer.batch_begin_turn(requests, &mut state);
            drop(renderer);

            let mut chunks_this_turn = 0_usize;
            loop {
                self.provision_scratch(requests, &state, output);
                let Some(mut renderer) = self.lock_renderer(mutex) else {
                    output.finish_rest(self, GpuAccumulate::Declined);
                    return;
                };
                let result = renderer.batch_step(requests, &mut output.scratches, &mut state);
                drop(renderer);
                #[cfg(test)]
                let result = {
                    steps_started += 1;
                    self.injected_failure(steps_started).map_or(result, Err)
                };
                let step = match result {
                    Ok(step) => step,
                    Err(error) => {
                        self.note_error(error);
                        output.finish_rest(self, GpuAccumulate::Declined);
                        return;
                    }
                };
                for event in step.events {
                    output.deliver(self, event);
                }
                if state.is_finished() {
                    return;
                }
                if step.dispatched {
                    chunks_this_turn += 1;
                }
                if chunks_this_turn < CHUNKS_PER_TURN {
                    continue;
                }
                if !self.turnstile.has_waiters_behind(ticket) {
                    // Nobody wants the renderer: keep the queue full, ask again later.
                    chunks_this_turn = 0;
                    continue;
                }
                // Somebody is waiting: drain our one in-flight chunk so the renderer is idle
                // for them, then rejoin the back of the queue.
                let Some(mut renderer) = self.lock_renderer(mutex) else {
                    output.finish_rest(self, GpuAccumulate::Declined);
                    return;
                };
                let drained = renderer.batch_yield(&mut output.scratches, &mut state);
                drop(renderer);
                match drained {
                    Ok(events) => {
                        for event in events {
                            output.deliver(self, event);
                        }
                    }
                    Err(error) => {
                        self.note_error(error);
                        output.finish_rest(self, GpuAccumulate::Declined);
                        return;
                    }
                }
                continue 'turns;
            }
        }
    }

    /// Makes sure the picture the next step starts has its zeroed scratch buffer.
    fn provision_scratch(
        &self,
        requests: &[TurnRequest<'_>],
        state: &BatchState<'_>,
        output: &mut BatchOutput<'_, '_>,
    ) {
        let index = state.next_item();
        let Some(request) = requests.get(index) else {
            return;
        };
        let pixels = request.scene.width as usize * request.scene.height as usize;
        if request.spp > 0 && pixels > 0 && output.scratches[index].len() != pixels {
            output.scratches[index] = self.take_scratch(pixels);
        }
    }

    /// Locks the renderer for one batch step. A poisoned mutex is never recovered (see
    /// `run_turns`): it marks the backend lost and yields `None`.
    fn lock_renderer<'m>(
        &self,
        mutex: &'m Mutex<GpuFrameRenderer>,
    ) -> Option<MutexGuard<'m, GpuFrameRenderer>> {
        let Ok(guard) = mutex.lock() else {
            tracing::warn!(
                "GPU renderer mutex poisoned (a previous turn panicked), disabling the GPU \
                 backend until a fresh device is acquired"
            );
            self.mark_lost("GPU renderer mutex poisoned (a previous turn panicked)".into());
            return None;
        };
        Some(guard)
    }

    /// Logs a batch step's failure like `run_turns` does: a lost device marks the backend
    /// lost, anything else is an ordinary decline.
    fn note_error(&self, error: GpuFrameError) {
        match error {
            GpuFrameError::DeviceLost(why) => {
                tracing::warn!(
                    "GPU device lost ({why}), disabling the GPU backend until a fresh \
                     device is acquired -- meanwhile every call falls back to the CPU \
                     tracer"
                );
                self.mark_lost(why);
            }
            other => {
                tracing::debug!("GPU declined this batch, using CPU tracer: {other}");
            }
        }
    }

    /// Records a device loss: remembers `reason`, starts the recovery cool-down, and
    /// only then raises [`Self::lost`], so a thread that sees the flag also finds the
    /// cool-down already running.
    fn mark_lost(&self, reason: String) {
        *self
            .last_lost_reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(reason);
        self.policy().record_loss(Instant::now());
        self.lost.store(true, Ordering::Release);
    }

    fn policy(&self) -> MutexGuard<'_, RecoveryPolicy> {
        self.recovery.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Tries to bring a lost GPU back, subject to the recovery policy: at least
    /// [`COOL_DOWN`] since the loss or the previous attempt, and at most six attempts
    /// per hour. Returns `true` when the backend is usable on return -- it was never lost,
    /// another thread already recovered it, or a fresh device was just acquired --
    /// and `false` when it is still lost (no attempt allowed yet, another thread is
    /// mid-attempt, or the attempt failed).
    ///
    /// [`Self::try_accumulate_cancellable`] calls this at the top of every request while
    /// lost; a caller that must report the backend's state first calls it directly. Never
    /// blocks behind another recovery: the losing thread just declines.
    #[must_use]
    pub fn try_recover(&self) -> bool {
        if !self.lost.load(Ordering::Acquire) {
            return true;
        }
        let Some(slot) = &self.renderer else {
            return false;
        };
        if !self.policy().may_attempt(Instant::now()) {
            return false;
        }
        if self.recovering.swap(true, Ordering::AcqRel) {
            return false;
        }
        let recovered = self.reacquire(slot);
        self.recovering.store(false, Ordering::Release);
        recovered
    }

    /// Acquires a fresh renderer and swaps it into `slot`. The slow part (adapter,
    /// device, megakernel compile) runs before any lock is taken; the swap itself then
    /// waits for a turnstile turn like a dispatch would, so it never lands inside
    /// another request's turn. A poisoned mutex is cleared by the swap.
    fn reacquire(&self, slot: &Mutex<GpuFrameRenderer>) -> bool {
        if !self.lost.load(Ordering::Acquire) {
            return true;
        }
        self.policy().record_attempt(Instant::now());
        let reason = self.last_lost_reason().unwrap_or_default();
        tracing::info!("GPU device was lost ({reason}); trying to acquire a fresh one");
        let mut fresh = match GpuFrameRenderer::new() {
            Ok(fresh) => fresh,
            Err(e) => {
                tracing::info!(
                    "GPU re-acquisition failed, staying on the CPU tracer for at least {} s: {e}",
                    COOL_DOWN.as_secs()
                );
                return false;
            }
        };

        let ticket = self.turnstile.take_ticket();
        let _turn = self.turnstile.wait_for_turn(ticket);
        let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
        fresh.set_pipeline_kind(guard.pipeline_kind());
        self.remember_label(fresh.adapter_label().to_owned());
        *guard = fresh;
        drop(guard);
        slot.clear_poison();
        self.policy().record_recovered();
        self.lost.store(false, Ordering::Release);
        let adapter = self
            .adapter_label()
            .unwrap_or_else(|| "unknown".to_string());
        tracing::info!(adapter = adapter.as_str(), "GPU render backend recovered");
        true
    }

    /// A zeroed buffer of `len` pixels from the pool (or a new one).
    fn take_scratch(&self, len: usize) -> Vec<Vec3> {
        let mut buffer = self
            .scratch_pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        buffer.clear();
        buffer.resize(len, Vec3::ZERO);
        buffer
    }

    /// Hands `buffer` back for the next request, unless the pool is already full.
    fn return_scratch(&self, buffer: Vec<Vec3>) {
        if buffer.capacity() == 0 {
            return;
        }
        let mut pool = self
            .scratch_pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if pool.len() < MAX_POOLED_SCRATCH {
            pool.push(buffer);
        }
    }

    /// The test seam behind [`Self::fail_on_turn`]: an injected loss exactly when `turn`
    /// is the armed turn number, disarming it.
    #[cfg(test)]
    fn injected_failure(&self, turn: usize) -> Option<GpuFrameError> {
        self.fail_on_turn
            .compare_exchange(turn, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
            .then(|| GpuFrameError::DeviceLost("injected failure (test seam)".to_string()))
    }
}
